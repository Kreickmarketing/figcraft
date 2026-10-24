//! A message bridge for a page that embeds FigCraft in an `<iframe>` on the same origin (Clearmark
//! keeps its file library there). Off when the page isn't in a frame.
//!
//! The parent posts `{type: "figcraft:request", id, action, ...}`; FigCraft answers each with
//! `{type: "figcraft:reply", id, ok, error?, ...}`. Actions:
//! - `open {name, bytes}`: open the file (`bytes` a `Uint8Array`) as a new tab;
//! - `getFile`: `{name, bytes, thumbnail, dirty}` for the file in front: its `.figcraft` bytes and a
//!   PNG of its first page (`thumbnail` null for an empty file);
//! - `exportPng {scale?}`: `{name, bytes}`, the page in front as one PNG;
//! - `status`: `{name, dirty, home}` (`name` null when no file is open).
//!
//! Once the app has started it posts `{type: "figcraft:ready"}`. Messages from other origins are
//! ignored. Requests are answered on the next frame, in order.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use base64::Engine as _;
use figcraft_ui_egui::FigApp;
use js_sys::{Object, Reflect, Uint8Array};
use serde_json::json;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

struct Request {
    id: JsValue,
    action: String,
    data: JsValue,
}

pub struct Bridge {
    parent: web_sys::Window,
    origin: String,
    queue: Rc<RefCell<VecDeque<Request>>>,
    _listener: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Bridge {
    /// Listen for the embedding page's requests (`ctx` is woken for each); None outside a frame.
    pub fn install(ctx: egui::Context) -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let (q, from) = (queue.clone(), origin.clone());
        let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if e.origin() != from {
                return;
            }
            let data = e.data();
            if string(&data, "type").as_deref() != Some("figcraft:request") {
                return;
            }
            let Some(action) = string(&data, "action") else { return };
            let id = Reflect::get(&data, &"id".into()).unwrap_or(JsValue::NULL);
            q.borrow_mut().push_back(Request { id, action, data });
            ctx.request_repaint();
        });
        window.add_event_listener_with_callback("message", listener.as_ref().unchecked_ref()).ok()?;
        let bridge = Self { parent, origin, queue, _listener: listener };
        bridge.post(&fields(&[("type", "figcraft:ready".into())]));
        Some(bridge)
    }

    /// Answer the requests that arrived since the last frame.
    pub fn handle(&self, fig: &mut FigApp) {
        loop {
            let Some(req) = self.queue.borrow_mut().pop_front() else { break };
            let reply = match answer(fig, &req) {
                Ok(reply) => {
                    set(&reply, "ok", true.into());
                    reply
                }
                Err(error) => fields(&[("ok", false.into()), ("error", error.into())]),
            };
            set(&reply, "type", "figcraft:reply".into());
            set(&reply, "id", req.id);
            self.post(&reply);
        }
    }

    fn post(&self, message: &Object) {
        if let Err(e) = self.parent.post_message(message, &self.origin) {
            log::error!("couldn't answer the embedding page: {e:?}");
        }
    }
}

fn answer(fig: &mut FigApp, req: &Request) -> Result<Object, String> {
    match req.action.as_str() {
        "open" => {
            let name = string(&req.data, "name").ok_or("open needs a name")?;
            let bytes = Reflect::get(&req.data, &"bytes".into())
                .ok()
                .and_then(|b| b.dyn_into::<Uint8Array>().ok())
                .ok_or("open needs bytes (a Uint8Array)")?
                .to_vec();
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            if fig.run_user("file.open", json!({"data": data, "name": name})).is_none() {
                return Err(format!("FigCraft couldn't open {name}"));
            }
            fig.ui.home = false;
            Ok(Object::new())
        }
        "getFile" => {
            let tab = figcraft_engine::branches::file_tab(&fig.session);
            let ds = fig.session.docs.get(tab).ok_or("no file is open")?;
            let bytes = figcraft_engine::cmd::file::file_bytes(ds).map_err(|e| e.to_string())?;
            let thumbnail = figcraft_engine::cmd::file::thumbnail(&ds.doc).map_or(JsValue::NULL, |t| Uint8Array::from(&t[..]).into());
            Ok(fields(&[
                ("name", ds.doc.name.as_str().into()),
                ("bytes", Uint8Array::from(&bytes[..]).into()),
                ("thumbnail", thumbnail),
                ("dirty", ds.is_dirty().into()),
            ]))
        }
        "exportPng" => {
            let scale = Reflect::get(&req.data, &"scale".into()).ok().and_then(|s| s.as_f64()).unwrap_or(1.0);
            let name = fig.session.doc().map(|d| format!("{}.png", d.name)).map_err(|e| e.to_string())?;
            let r = fig.run("file.export", json!({"format": "png", "scale": scale})).ok_or("FigCraft couldn't export the page")?;
            let bytes = r["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok()).ok_or("the export came back empty")?;
            Ok(fields(&[("name", name.into()), ("bytes", Uint8Array::from(&bytes[..]).into())]))
        }
        "status" => {
            let tab = figcraft_engine::branches::file_tab(&fig.session);
            let ds = fig.session.docs.get(tab);
            Ok(fields(&[
                ("name", ds.map_or(JsValue::NULL, |d| d.doc.name.as_str().into())),
                ("dirty", ds.is_some_and(|d| d.is_dirty()).into()),
                ("home", fig.ui.home.into()),
            ]))
        }
        other => Err(format!("unknown action {other}")),
    }
}

fn string(obj: &JsValue, key: &str) -> Option<String> {
    Reflect::get(obj, &key.into()).ok().and_then(|v| v.as_string())
}

fn set(obj: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(obj, &key.into(), &value);
}

fn fields(pairs: &[(&str, JsValue)]) -> Object {
    let obj = Object::new();
    for (key, value) in pairs {
        set(&obj, key, value.clone());
    }
    obj
}

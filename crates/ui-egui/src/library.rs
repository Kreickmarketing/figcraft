//! A file library kept by the page that embeds FigCraft (Clearmark's): its files on the home screen
//! as thumbnail cards or a list, opened on click, and Save handing the file to the page instead of
//! downloading it. Off (`FigApp::library` is `None`) until the page sends its files.
//!
//! The page and the app talk through [`Library::outbox`] (what the app asks of the page, taken each
//! frame by the shell) and the shell's calls into [`Library::set_files`] and [`Library::saved`].

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use figcraft_doc::Document;

use crate::FigApp;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets;

/// A file in the library, as the page describes it.
pub struct LibraryFile {
    pub id: String,
    pub name: String,
    /// The line under the name ("Edited 2 days ago").
    pub detail: String,
    /// A PNG of the file's first page.
    pub thumb: Option<Vec<u8>>,
}

/// What the app asks of the page.
pub enum LibraryRequest {
    /// Open this file (the page sends its bytes back to open).
    Open { id: String },
    /// Keep the document `uid` as the library file `id` (None: a new file).
    Save { uid: u64, id: Option<String>, name: String, bytes: Vec<u8>, thumbnail: Option<Vec<u8>> },
    /// Send the list again.
    Refresh,
    /// Go back to the page's own apps.
    Leave,
}

#[derive(Default)]
pub struct Library {
    /// The section's name in the home sidebar.
    pub title: String,
    pub files: Vec<LibraryFile>,
    /// Shown as a list rather than cards.
    pub list_view: bool,
    pub outbox: Vec<LibraryRequest>,
    /// The library file each open document came from or was saved as, by document uid.
    pub origin: BTreeMap<u64, String>,
    /// Documents being saved: the version handed over, so a save that arrives after more edits
    /// leaves those shown as unsaved.
    saving: BTreeMap<u64, Arc<Document>>,
    /// Card pictures by file id and the size of the PNG they came from (a new save, a new picture).
    textures: BTreeMap<(String, usize), Option<egui::TextureHandle>>,
}

impl Library {
    pub fn set_files(&mut self, files: Vec<LibraryFile>) {
        self.textures.retain(|(id, len), _| files.iter().any(|f| &f.id == id && f.thumb.as_ref().map_or(0, Vec::len) == *len));
        self.files = files;
    }
}

/// Save the file in front to the library (`copy`: as a new file even if it came from there).
/// False when there is no library, for the caller to save as it otherwise would.
pub(crate) fn save(app: &mut FigApp, copy: bool) -> bool {
    let Some(library) = app.library.as_mut() else { return false };
    let tab = figcraft_engine::branches::file_tab(&app.session);
    let Some(ds) = app.session.docs.get(tab) else { return true };
    let bytes = match figcraft_engine::cmd::file::file_bytes(ds) {
        Ok(bytes) => bytes,
        Err(e) => {
            app.toast(e.to_string());
            return true;
        }
    };
    let id = if copy { None } else { library.origin.get(&ds.uid).cloned() };
    library.saving.insert(ds.uid, ds.doc.clone());
    library.outbox.push(LibraryRequest::Save {
        uid: ds.uid,
        id,
        name: ds.doc.name.clone(),
        bytes,
        thumbnail: figcraft_engine::cmd::file::thumbnail(&ds.doc),
    });
    app.toast(tr("Saving to the Library…"));
    true
}

/// The page kept document `uid` as library file `id`: it counts as saved unless it changed since.
pub fn saved(app: &mut FigApp, uid: u64, id: String) {
    let Some(library) = app.library.as_mut() else { return };
    library.origin.insert(uid, id);
    let handed_over = library.saving.remove(&uid);
    if let Some(ds) = app.session.docs.iter_mut().find(|d| d.uid == uid)
        && handed_over.is_some_and(|doc| Arc::ptr_eq(&doc, &ds.doc))
    {
        ds.mark_saved();
    }
    app.toast(tr("Saved to the Library"));
}

/// A save the page couldn't finish: the document stays unsaved.
pub fn save_failed(app: &mut FigApp, uid: u64) {
    if let Some(library) = app.library.as_mut() {
        library.saving.remove(&uid);
    }
}

const CARD_W: f32 = 248.0;
const THUMB_H: f32 = 148.0;
const CARD_H: f32 = THUMB_H + 64.0;
const GAP: f32 = 20.0;

/// The home screen's library section: a heading with the grid/list switch, then the files.
pub(crate) fn section(app: &mut FigApp, ui: &mut Ui, t: &Tokens) {
    let Some(library) = app.library.as_mut() else { return };
    let mut chosen = None;
    let mut new_file = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&library.title).font(semibold(18.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (list, icon, label) in [(true, Icon::Menu, tr("List view")), (false, Icon::Grid, tr("Grid view"))] {
                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), Sense::click());
                let resp = resp.on_hover_text(label);
                crate::a11y::tab(&resp, label, library.list_view == list);
                if library.list_view == list || resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(5), if library.list_view == list { t.selected_row } else { t.hover });
                }
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), icon, t.icon);
                if resp.clicked() {
                    library.list_view = list;
                }
            }
            ui.add_space(8.0);
            if widgets::button(ui, tr("New design file"), false, true).clicked() {
                new_file = true;
            }
        });
    });
    ui.add_space(24.0);
    if library.files.is_empty() {
        ui.label(egui::RichText::new(tr("Nothing saved yet")).font(semibold(13.0)).color(t.text));
        ui.add_space(6.0);
        widgets::label(ui, tr("Save a design (File ▸ Save) to keep it here."), t.text_secondary);
    } else if library.list_view {
        for file in &library.files {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(760.0), 40.0), Sense::hover());
            let resp = ui.interact(r, Id::new(("library_row", &file.id)), Sense::click());
            crate::a11y::button(&resp, &file.name);
            let row = crate::home::FileRow {
                mark: Some((Icon::Frame, t.icon_secondary)),
                name: &file.name,
                ink: t.text,
                detail: &file.detail,
                detail_size: 11.0,
                detail_ink: t.text_secondary,
            };
            crate::home::file_row(ui, t, r, &resp, &row);
            if resp.clicked() {
                chosen = Some(file.id.clone());
            }
        }
    } else {
        let columns = (((ui.available_width() + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        for row in library.files.chunks(columns) {
            ui.horizontal(|ui| {
                for file in row {
                    let key = (file.id.clone(), file.thumb.as_ref().map_or(0, Vec::len));
                    let texture = library.textures.entry(key).or_insert_with(|| picture(ui.ctx(), file)).clone();
                    if card(ui, t, file, texture.as_ref()) {
                        chosen = Some(file.id.clone());
                    }
                }
            });
            ui.add_space(GAP);
        }
    }
    if let Some(id) = chosen {
        library.outbox.push(LibraryRequest::Open { id });
        ui.ctx().request_repaint();
    }
    if new_file && app.run("file.new", serde_json::json!({})).is_some() {
        app.ui.home = false;
    }
}

fn picture(ctx: &egui::Context, file: &LibraryFile) -> Option<egui::TextureHandle> {
    let rgba = image::load_from_memory(file.thumb.as_ref()?).ok()?.to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    Some(ctx.load_texture(format!("library-thumb-{}", file.id), image, egui::TextureOptions::LINEAR))
}

fn card(ui: &mut Ui, t: &Tokens, file: &LibraryFile, texture: Option<&egui::TextureHandle>) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
    crate::a11y::button(&resp, &file.name);
    ui.painter().rect_filled(r, CornerRadius::same(8), if resp.hovered() { t.hover } else { t.panel });
    ui.painter().rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let picture = Rect::from_min_size(r.min, vec2(CARD_W, THUMB_H)).shrink(1.0);
    let top = CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 };
    ui.painter().rect_filled(picture, top, t.field);
    match texture {
        Some(texture) => {
            let size = texture.size_vec2();
            let inner = picture.shrink(10.0);
            let fit = (inner.width() / size.x).min(inner.height() / size.y).min(1.0);
            let image = Rect::from_center_size(inner.center(), size * fit);
            ui.painter().image(texture.id(), image, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => icons::paint(ui.painter(), Rect::from_center_size(picture.center(), vec2(20.0, 20.0)), Icon::Frame, t.icon_secondary),
    }
    ui.painter().line_segment([picture.left_bottom(), picture.right_bottom()], Stroke::new(1.0, t.border));
    let (x, w) = (r.min.x + 14.0, CARD_W - 28.0);
    ui.painter().text(pos2(x, picture.max.y + 22.0), Align2::LEFT_CENTER, widgets::elide_font(ui, &file.name, w, semibold(12.0)), semibold(12.0), t.text);
    ui.painter().text(pos2(x, picture.max.y + 42.0), Align2::LEFT_CENTER, widgets::elide_font(ui, &file.detail, w, regular(11.0)), regular(11.0), t.text_secondary);
    resp.clicked()
}

/// The page's own button at the left of the tab bar (Clearmark's mark): back to its apps.
/// Returns the width it took.
pub(crate) fn tab_bar_button(app: &mut FigApp, ui: &mut Ui, x: f32, rect: Rect, height: f32) -> f32 {
    let Some(library) = app.library.as_mut() else { return 0.0 };
    let t = Tokens::get(ui.ctx());
    let r = Rect::from_min_size(pos2(x, rect.min.y), vec2(44.0, height));
    let resp = ui.interact(r, ui.id().with("library_leave"), Sense::click()).on_hover_text(tr("Back to Clearmark apps"));
    crate::a11y::button(&resp, tr("Back to Clearmark apps"));
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::ZERO, t.hover);
    }
    // Clearmark's mark: "AI" on a coral square.
    let mark = Rect::from_center_size(r.center(), vec2(22.0, 22.0));
    ui.painter().rect_filled(mark, CornerRadius::same(5), Color32::from_rgb(0xF2, 0x5C, 0x4C));
    ui.painter().text(mark.center(), Align2::CENTER_CENTER, "AI", semibold(11.0), Color32::WHITE);
    if resp.clicked() {
        library.outbox.push(LibraryRequest::Leave);
    }
    ui.painter().line_segment([pos2(r.max.x, rect.min.y), pos2(r.max.x, rect.max.y)], Stroke::new(1.0, t.border));
    44.0
}

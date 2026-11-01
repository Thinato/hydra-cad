//! Proposals tab: the review queue for catalogue entries an AI assistant drafted from a
//! datasheet through `hydra-mcp`. The engineer checks the rendered symbol against the
//! datasheet, may correct the valve type's fields, then approves or rejects with a reason.
//! Only approval writes catalogue rows.

use eframe::egui::{self, Color32, FontFamily, RichText, Stroke, Vec2};
use hydra_core::{Catalogue, CatalogueRef, Proposal, SymbolDef};
use hydra_db::{ProposalRecord, ProposalStatus};
use hydra_symbols::render_symbol;

use crate::catalogue::Shared;
use crate::db::Request;
use crate::schematic::draw_fitted;
use crate::theme::{self, LIGHT, Mark};

#[derive(Default)]
pub struct ProposalsPanel {
    /// `None` until the first list arrives.
    list: Option<Vec<ProposalRecord>>,
    show_reviewed: bool,
    review: Option<Review>,
    /// A list, approve or reject request is waiting for its reply.
    busy: bool,
    error: Option<String>,
    /// Each listed proposal's check of its draft for its queue row (`None` once reviewed),
    /// with the catalogue size and list length it was made for. Cleared when a list arrives.
    row_checks: Option<RowChecks>,
}

/// A queue row's check: its mark and words, or `None` once the proposal is reviewed.
type RowCheck = Option<(Mark, String)>;
/// Every row's check, keyed by the catalogue size and list length they were made for.
type RowChecks = (([usize; 4], usize), Vec<RowCheck>);

/// How many rows the catalogue has of each kind. Rows are only ever added, so a change in
/// these tells that checks against it may have changed.
fn catalogue_size(c: &Catalogue) -> [usize; 4] {
    [
        c.manufacturers.len(),
        c.symbol_defs.len(),
        c.valve_types.len(),
        c.parts.len(),
    ]
}

/// What a waiting proposal's queue row says about it: its problems, that it replaces a
/// released valve type, or that its checks pass.
/// `problems` counts what stops approval, of the draft or of the reviewer's edit.
fn row_check(p: &Proposal, problems: usize, catalogue: &Catalogue) -> (Mark, String) {
    let n = problems;
    let base = catalogue.valve_type(&CatalogueRef::new(&p.valve_type.key, p.valve_type.version));
    if n > 0 {
        let s = if n == 1 { "" } else { "s" };
        (Mark::Error, format!("{n} problem{s}"))
    } else if base.is_some_and(|b| b.symbol.is_some()) {
        (Mark::Warning, "Replaces a released type".into())
    } else {
        (Mark::Check, "Checks pass".into())
    }
}

/// The selected proposal, with the reviewer's edits.
struct Review {
    record: ProposalRecord,
    series: String,
    code: String,
    mounting: String,
    params: String,
    note: String,
    /// Problems and resulting parts, kept until the edits or the catalogue change.
    checked: Option<(String, Checked)>,
}

struct Checked {
    problems: Vec<String>,
    /// Every part of the new version: number, options, and "new", "replaced" or "kept".
    parts: Vec<(String, String, &'static str)>,
}

impl Checked {
    fn new(edited: &Result<Proposal, String>, catalogue: &Catalogue) -> Self {
        let p = match edited {
            Ok(p) => p,
            Err(e) => {
                return Checked {
                    problems: vec![e.clone()],
                    parts: Vec::new(),
                };
            }
        };
        let base = CatalogueRef::new(&p.valve_type.key, p.valve_type.version);
        let target = p.target();
        let after = p.apply(catalogue);
        let parts = after
            .parts_of(&target)
            .map(|part| {
                let in_base = catalogue
                    .parts_of(&base)
                    .any(|b| b.part_number == part.part_number);
                let proposed = p.parts.iter().any(|d| d.part_number == part.part_number);
                let how = match (in_base, proposed) {
                    (false, _) => "new",
                    (true, true) => "replaced",
                    (true, false) => "kept",
                };
                let options = if part.options.is_null() {
                    String::new()
                } else {
                    serde_json::to_string(&part.options).unwrap_or_default()
                };
                (part.part_number.clone(), options, how)
            })
            .collect();
        Checked {
            problems: p.problems(catalogue),
            parts,
        }
    }
}

impl Review {
    fn new(record: ProposalRecord) -> Self {
        let shown = record.approved.as_ref().unwrap_or(&record.proposal);
        let v = &shown.valve_type;
        Review {
            series: v.series.clone(),
            code: v.code.clone(),
            mounting: v.mounting.clone().unwrap_or_default(),
            params: if v.params.is_null() {
                String::new()
            } else {
                serde_json::to_string_pretty(&v.params).unwrap_or_default()
            },
            note: String::new(),
            checked: None,
            record,
        }
    }

    /// Checks of the edited proposal against `catalogue`, rerun only when either changes.
    /// Catalogue rows are only ever added, so their counts tell when it changed.
    fn checked(&mut self, catalogue: &Catalogue) -> &Checked {
        let key = format!(
            "{}\0{}\0{}\0{}\0{:?}",
            self.series,
            self.code,
            self.mounting,
            self.params,
            catalogue_size(catalogue)
        );
        if self.checked.as_ref().is_none_or(|(k, _)| *k != key) {
            let checked = Checked::new(&self.edited(), catalogue);
            self.checked = Some((key, checked));
        }
        &self.checked.as_ref().expect("just checked").1
    }

    /// The proposal with the reviewer's edits, or why the edits don't parse.
    fn edited(&self) -> Result<Proposal, String> {
        let mut p = self.record.proposal.clone();
        let v = &mut p.valve_type;
        v.series = self.series.trim().to_string();
        v.code = self.code.trim().to_string();
        v.mounting = Some(self.mounting.trim().to_string()).filter(|m| !m.is_empty());
        v.params = if self.params.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&self.params)
                .map_err(|e| format!("Parameters aren't JSON: {e}"))?
        };
        Ok(p)
    }

    fn pending(&self) -> bool {
        self.record.status == ProposalStatus::Pending
    }
}

impl ProposalsPanel {
    /// The worker's answer to [`Request::ListProposals`].
    pub fn listed(&mut self, list: Vec<ProposalRecord>) {
        self.busy = false;
        self.error = None;
        // Keep the reviewer's edits if their proposal is still there and unchanged.
        if let Some(r) = &self.review {
            match list.iter().find(|p| p.id == r.record.id) {
                Some(p) if p.status == r.record.status => {}
                Some(p) => self.review = Some(Review::new(p.clone())),
                None => self.review = None,
            }
        }
        self.list = Some(list);
        self.row_checks = None;
    }

    /// An approve or reject went through; the list is fetched again.
    pub fn reviewed(&mut self, shared: &Shared) {
        self.busy = false;
        self.refresh(shared);
    }

    pub fn failed(&mut self, message: String) {
        self.busy = false;
        self.error = Some(message);
    }

    fn refresh(&mut self, shared: &Shared) {
        if let Some(db) = &shared.db {
            db.send(Request::ListProposals {
                pending_only: !self.show_reviewed,
            });
            self.busy = true;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        if shared.db.is_none() {
            ui.weak("Proposals need the shared database.");
            return;
        }
        // Load once on first view; after a failure, Refresh retries.
        if self.list.is_none() && !self.busy && self.error.is_none() {
            self.refresh(shared);
        }
        egui::Panel::left("proposal-list")
            .default_size(300.0)
            .min_size(220.0)
            .resizable(true)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(0))
            .show(ui, |ui| self.queue_ui(ui, shared));

        ui.painter()
            .rect_filled(ui.available_rect_before_wrap(), 0.0, LIGHT.surface);
        if let Some(e) = &self.error {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(28, 8))
                .show(ui, |ui| theme::marked(ui, Mark::Error, e.as_str()));
        }
        match self.review.as_mut() {
            Some(review) => {
                if let Some(request) = detail(ui, review, shared, self.busy)
                    && let Some(db) = &shared.db
                {
                    // Opening the datasheet has no reply to wait for.
                    if matches!(
                        request,
                        Request::ApproveProposal { .. } | Request::RejectProposal { .. }
                    ) {
                        self.busy = true;
                        self.error = None;
                    }
                    db.send(request);
                }
            }
            None => {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(28, 20))
                    .show(ui, |ui| {
                        ui.weak(
                            "Select a proposal. An AI assistant connected through hydra-mcp \
                             drafts them from the datasheets attached to valve types.",
                        );
                    });
            }
        }
    }

    /// The review queue: its header, the Waiting/All switch and one row per proposal.
    fn queue_ui(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let header = egui::Frame::new()
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("REVIEW QUEUE").text_style(theme::overline()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let refresh = ui
                            .add_enabled(!self.busy, egui::Button::new("Refresh"))
                            .on_hover_text("Fetch the queue again.")
                            .on_disabled_hover_text("Waiting for the database.");
                        if refresh.clicked() {
                            self.refresh(shared);
                        }
                        if self.busy {
                            ui.spinner();
                        }
                    });
                });
                // Two joined segments: waiting proposals only, or every proposal.
                egui::Frame::new()
                    .fill(LIGHT.surface)
                    .stroke(Stroke::new(1.0, LIGHT.border_strong))
                    .corner_radius(theme::RADIUS)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal(|ui| {
                            let half = Vec2::new(ui.available_width() / 2.0, 28.0);
                            for (all, word) in [(false, "Waiting"), (true, "All")] {
                                let selected = self.show_reviewed == all;
                                let segment = ui.add_enabled(
                                    !self.busy,
                                    egui::Button::selectable(selected, word)
                                        .corner_radius(0)
                                        .min_size(half),
                                );
                                if segment.clicked() && !selected {
                                    self.show_reviewed = all;
                                    self.refresh(shared);
                                }
                            }
                        });
                    });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            header.response.rect.bottom(),
            Stroke::new(1.0, LIGHT.border),
        );

        let Some(list) = &self.list else {
            return;
        };
        let catalogue = &shared.catalogue;
        let key = (catalogue_size(catalogue), list.len());
        if self.row_checks.as_ref().is_none_or(|(k, _)| *k != key) {
            let checks = list
                .iter()
                .map(|p| {
                    let pending = p.status == ProposalStatus::Pending;
                    let problems = || p.proposal.problems(catalogue).len();
                    pending.then(|| row_check(&p.proposal, problems(), catalogue))
                })
                .collect();
            self.row_checks = Some((key, checks));
        }
        let checks = &self.row_checks.as_ref().expect("just checked").1;
        // The selected row follows the reviewer's edits, as the action bar does.
        let edited = self.review.as_mut().filter(|r| r.pending()).map(|r| {
            let problems = r.checked(catalogue).problems.len();
            let check = row_check(&r.record.proposal, problems, catalogue);
            (r.record.id.clone(), check)
        });
        if list.is_empty() {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.weak(if self.show_reviewed {
                        "No proposals yet."
                    } else {
                        "No proposals waiting for review."
                    });
                });
        }
        let mut picked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (p, check) in list.iter().zip(checks) {
                let selected = self.review.as_ref().is_some_and(|r| r.record.id == p.id);
                let check = match &edited {
                    Some((id, edited)) if *id == p.id => Some(edited),
                    _ => check.as_ref(),
                };
                if queue_row(ui, p, check, selected, catalogue).clicked() {
                    picked = Some(p.clone());
                }
            }
        });
        if let Some(p) = picked {
            self.review = Some(Review::new(p));
        }
    }
}

/// The chip of a review status.
fn status_chip(status: ProposalStatus) -> (Mark, &'static str) {
    match status {
        ProposalStatus::Pending => (Mark::Waiting, "Waiting for review"),
        ProposalStatus::Approved => (Mark::Check, "Approved"),
        ProposalStatus::Rejected => (Mark::Rejected, "Rejected"),
    }
}

fn manufacturer_name<'a>(catalogue: &'a Catalogue, key: &'a str) -> &'a str {
    catalogue
        .manufacturers
        .iter()
        .find(|m| m.key == key)
        .map_or(key, |m| m.name.as_str())
}

/// One proposal in the queue: code and time, target, who drafted it, then its check while it
/// waits or its decision once reviewed. The selected row is tinted with an accent edge; a
/// focused one gets the edge too. Screen readers and kittests read it as "{series} {code}".
fn queue_row(
    ui: &mut egui::Ui,
    p: &ProposalRecord,
    check: Option<&(Mark, String)>,
    selected: bool,
    catalogue: &Catalogue,
) -> egui::Response {
    let v = &p.proposal.valve_type;
    let name = format!("{} {}", v.series, v.code);
    let row = ui
        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            // Clicks on the row's text select the row.
            ui.style_mut().interaction.selectable_labels = false;
            let last = ui.response();
            let fill = if selected {
                LIGHT.accent_tint
            } else if last.hovered() {
                LIGHT.hover
            } else {
                Color32::TRANSPARENT
            };
            let edge = if selected || last.has_focus() {
                LIGHT.accent
            } else {
                Color32::TRANSPARENT
            };
            let row = egui::Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, edge))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        let code = RichText::new(&name)
                            .family(FontFamily::Name(theme::MONO_SEMIBOLD.into()));
                        ui.label(code);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(&p.created_at).small().color(LIGHT.secondary));
                        });
                    });
                    ui.monospace(p.proposal.target().to_string());
                    ui.label(
                        RichText::new(format!(
                            "{} · by {}",
                            manufacturer_name(catalogue, &v.manufacturer),
                            p.created_by
                        ))
                        .small()
                        .color(LIGHT.secondary),
                    );
                    match check {
                        Some((mark, text)) => {
                            let text = RichText::new(text)
                                .small()
                                .family(FontFamily::Name(theme::SEMIBOLD.into()));
                            theme::marked(ui, *mark, text);
                        }
                        None => {
                            let (mark, word) = status_chip(p.status);
                            theme::chip(ui, mark, word);
                        }
                    }
                });
            let rect = row.response.rect;
            ui.painter().hline(
                rect.x_range(),
                rect.bottom(),
                Stroke::new(1.0, LIGHT.divider),
            );
        })
        .response;
    row.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
    });
    row
}

/// A heading over a section of the detail view, with space above it.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(16.0);
    ui.label(RichText::new(title).text_style(theme::heading()));
}

/// A bordered table: square, with `rows` laid out flush.
fn table(ui: &mut egui::Ui, width: f32, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(LIGHT.surface)
        .stroke(Stroke::new(1.0, LIGHT.border))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
}

/// One table row on `fill`, with a divider under it.
fn row(ui: &mut egui::Ui, fill: Color32, cells: impl FnOnce(&mut egui::Ui)) {
    let r = egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(12.0, 4.0);
            ui.horizontal_top(cells);
        });
    let rect = r.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, LIGHT.divider),
    );
}

/// A table cell `width` wide.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// One proposal: header, symbol, fields, parts, notes and, while it waits, the action bar.
/// Returns a request to send.
fn detail(ui: &mut egui::Ui, review: &mut Review, shared: &Shared, busy: bool) -> Option<Request> {
    let catalogue = &shared.catalogue;
    let pending = review.pending();
    let edited = review.edited();
    let proposal = edited
        .clone()
        .unwrap_or_else(|_| review.record.proposal.clone());
    let v = &proposal.valve_type;
    // What the proposal changes, when it updates an existing valve type.
    let base = catalogue
        .valve_type(&CatalogueRef::new(&v.key, v.version))
        .cloned();
    let mut request = None;

    if pending {
        let id = egui::Id::new("proposal-actions");
        let height = |ctx: &egui::Context| egui::PanelState::load(ctx, id).map(|s| s.size().y);
        let before = height(ui.ctx());
        egui::Panel::bottom(id)
            .resizable(false)
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(28, 12)),
            )
            .show(ui, |ui| {
                if let Some(r) = actions(ui, review, &edited, catalogue, busy) {
                    request = Some(r);
                }
            });
        // The bar is placed at last frame's height: redo the frame when a problem came or went.
        if height(ui.ctx()) != before {
            ui.ctx().request_discard("the action bar changed height");
        }
    }

    let symbol: Option<&SymbolDef> = match (&proposal.new_symbol, &v.symbol) {
        (Some(new), _) => Some(&new.definition),
        (None, Some(s)) => catalogue.symbol_def(s),
        (None, None) => None,
    };
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(28, 20))
                .show(ui, |ui| {
                    let width = ui.available_width().clamp(0.0, 760.0);
                    ui.set_max_width(width);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let (mark, word) = status_chip(review.record.status);
                                theme::chip(ui, mark, word);
                                theme::chip(ui, Mark::Agent, "Assistant draft");
                            });
                            ui.heading(format!("{} {}", v.series, v.code));
                            ui.monospace(format!("{} → {}", v.key, proposal.target()));
                            let caption = |ui: &mut egui::Ui, text: String| {
                                ui.label(RichText::new(text).small().color(LIGHT.secondary));
                            };
                            caption(
                                ui,
                                format!(
                                    "Drafted by {} on {}",
                                    review.record.created_by, review.record.created_at
                                ),
                            );
                            if let Some(by) = &review.record.reviewed_by {
                                let when = review.record.reviewed_at.as_deref().unwrap_or("");
                                caption(ui, format!("Reviewed by {by} on {when}"));
                            }
                            if let Some(note) = &review.record.review_note {
                                ui.label(format!("Reason: {note}"));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| match (
                            &review.record.datasheet_sha256,
                            &shared.db,
                        ) {
                            (Some(sha256), Some(_)) => {
                                let open = egui::Button::new("Open datasheet")
                                    .min_size(Vec2::new(0.0, 32.0));
                                if ui.add(open).clicked() {
                                    request = Some(Request::OpenAsset {
                                        sha256: sha256.clone(),
                                        extension: "pdf",
                                    });
                                }
                            }
                            _ => {
                                ui.weak("No datasheet attached.");
                            }
                        });
                    });

                    ui.add_space(16.0);
                    symbol_preview(ui, symbol, &proposal);

                    if pending && let Some(b) = base.as_ref().filter(|b| b.symbol.is_some()) {
                        ui.add_space(16.0);
                        theme::banner(
                            ui,
                            Mark::Warning,
                            &format!(
                                "This replaces {}, which is not a draft and may be in use.",
                                b.reference()
                            ),
                            "Check every change.",
                        );
                    }

                    section(ui, "Fields");
                    fields(ui, review, v, base.as_ref(), catalogue, pending, width);

                    let checked = review.checked(catalogue);
                    if !checked.parts.is_empty() {
                        section(
                            ui,
                            &format!("Parts of the new version ({})", checked.parts.len()),
                        );
                        table(ui, width, |ui| {
                            for (number, options, how) in &checked.parts {
                                row(ui, Color32::TRANSPARENT, |ui| {
                                    cell(ui, 200.0, |ui| {
                                        ui.label(
                                            RichText::new(number).family(FontFamily::Name(
                                                theme::MONO_SEMIBOLD.into(),
                                            )),
                                        );
                                    });
                                    cell(ui, 64.0, |ui| {
                                        ui.label(RichText::new(*how).color(LIGHT.secondary));
                                    });
                                    ui.label(RichText::new(options).color(LIGHT.secondary));
                                });
                            }
                        });
                    }
                    if !proposal.notes.is_empty() {
                        section(ui, "Assistant's notes");
                        egui::Frame::new()
                            .fill(LIGHT.panel)
                            .stroke(Stroke::new(1.0, LIGHT.border))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width((width - 24.0).max(0.0));
                                ui.label(&proposal.notes);
                            });
                    }
                });
        });
    request
}

/// The symbol as Hydra will draw it, on the sheet, with its reference in a caption strip.
fn symbol_preview(ui: &mut egui::Ui, symbol: Option<&SymbolDef>, proposal: &Proposal) {
    let size = Vec2::new(360.0, 200.0);
    egui::Frame::new()
        .fill(LIGHT.sheet)
        .stroke(Stroke::new(1.0, LIGHT.border))
        .show(ui, |ui| {
            ui.set_width(size.x);
            ui.spacing_mut().item_spacing.y = 0.0;
            match symbol {
                Some(def) => draw_fitted(ui, size, &render_symbol(def).primitives),
                None => {
                    ui.allocate_exact_size(size, egui::Sense::hover());
                }
            }
            let caption = egui::Frame::new()
                .fill(LIGHT.panel)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("How Hydra will draw it")
                                .small()
                                .family(FontFamily::Name(theme::SEMIBOLD.into())),
                        );
                        let reference = match (&proposal.new_symbol, &proposal.valve_type.symbol) {
                            (Some(new), _) => Some(format!("new {}", new.reference())),
                            (None, Some(s)) => Some(s.to_string()),
                            (None, None) => None,
                        };
                        match reference {
                            Some(reference) => {
                                let right = egui::Layout::right_to_left(egui::Align::Center);
                                ui.with_layout(right, |ui| {
                                    ui.label(
                                        RichText::new(reference)
                                            .text_style(egui::TextStyle::Monospace)
                                            .color(LIGHT.secondary),
                                    );
                                });
                            }
                            None => {
                                let right = egui::Layout::right_to_left(egui::Align::Center);
                                ui.with_layout(right, |ui| {
                                    theme::marked(
                                        ui,
                                        Mark::Error,
                                        RichText::new("No symbol").small(),
                                    );
                                });
                            }
                        }
                    });
                });
            let top = caption.response.rect.top();
            ui.painter().hline(
                caption.response.rect.x_range(),
                top,
                Stroke::new(1.0, LIGHT.border),
            );
        });
}

/// The fields table: what the proposal sets, editable while it waits, beside what it was.
fn fields(
    ui: &mut egui::Ui,
    review: &mut Review,
    v: &hydra_core::ValveType,
    base: Option<&hydra_core::ValveType>,
    catalogue: &Catalogue,
    pending: bool,
    width: f32,
) {
    // Field, value and was; the row's 12 px margins and gaps come off the table's width.
    let cols = [150.0, ((width - 150.0 - 72.0) * 0.6).max(0.0)];
    let header = |ui: &mut egui::Ui, text: &str| {
        ui.label(
            RichText::new(text)
                .text_style(theme::overline())
                .color(LIGHT.secondary),
        );
    };
    // One row: the label, a CHANGED chip when the value differs from the base, the value, and
    // what it was.
    let field = |ui: &mut egui::Ui,
                 label: &str,
                 old: Option<String>,
                 new: &str,
                 value: &mut dyn FnMut(&mut egui::Ui)| {
        let old = old.filter(|o| o != new);
        let fill = if old.is_some() {
            LIGHT.unlisted.tint
        } else {
            Color32::TRANSPARENT
        };
        row(ui, fill, |ui| {
            cell(ui, cols[0], |ui| {
                ui.label(RichText::new(label).text_style(theme::label()));
                if old.is_some() {
                    theme::chip(ui, Mark::Warning, "Changed");
                }
            });
            cell(ui, cols[1], |ui| value(ui));
            if let Some(old) = old {
                ui.label(
                    RichText::new(format!(
                        "was {}",
                        if old.is_empty() { "empty" } else { &old }
                    ))
                    .color(LIGHT.secondary),
                );
            }
        });
    };
    table(ui, width, |ui| {
        row(ui, LIGHT.panel, |ui| {
            cell(ui, cols[0], |ui| header(ui, "FIELD"));
            cell(ui, cols[1], |ui| {
                header(
                    ui,
                    if pending {
                        "VALUE · EDIT BEFORE APPROVING"
                    } else {
                        "VALUE"
                    },
                )
            });
            header(ui, "WAS");
        });
        field(ui, "Manufacturer", None, "", &mut |ui| {
            ui.label(manufacturer_name(catalogue, &v.manufacturer));
        });
        let edit = |ui: &mut egui::Ui, value: &mut String| {
            ui.add_enabled(
                pending,
                egui::TextEdit::singleline(value)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        };
        let series = review.series.clone();
        field(
            ui,
            "Series",
            base.map(|b| b.series.clone()),
            &series,
            &mut |ui| edit(ui, &mut review.series),
        );
        let code = review.code.clone();
        field(ui, "Code", base.map(|b| b.code.clone()), &code, &mut |ui| {
            edit(ui, &mut review.code)
        });
        let mounting = review.mounting.clone();
        field(
            ui,
            "Mounting",
            base.map(|b| b.mounting.clone().unwrap_or_default()),
            &mounting,
            &mut |ui| edit(ui, &mut review.mounting),
        );
        let symbol = v.symbol.as_ref().map(|s| s.to_string()).unwrap_or_default();
        field(
            ui,
            "Symbol",
            base.map(|b| b.symbol.as_ref().map(|s| s.to_string()).unwrap_or_default()),
            &symbol,
            &mut |ui| {
                ui.monospace(&symbol);
            },
        );
        let params = |p: &serde_json::Value| {
            if p.is_null() {
                String::new()
            } else {
                serde_json::to_string(p).unwrap_or_default()
            }
        };
        field(
            ui,
            "Parameters",
            base.map(|b| params(&b.params)),
            &params(&v.params),
            &mut |ui| {
                ui.add_enabled(
                    pending,
                    egui::TextEdit::multiline(&mut review.params)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            },
        );
    });
}

/// The action bar of a waiting proposal: the reason and Reject, then what blocks approval and
/// Approve. Returns the review request to send.
fn actions(
    ui: &mut egui::Ui,
    review: &mut Review,
    edited: &Result<Proposal, String>,
    catalogue: &Catalogue,
    busy: bool,
) -> Option<Request> {
    let mut request = None;
    // Checked against the catalogue as it is now: someone may have saved since.
    let problems = review.checked(catalogue).problems.clone();
    for p in &problems {
        theme::marked(ui, Mark::Error, p.as_str());
    }
    ui.horizontal(|ui| {
        // Centre everything on the 32 px buttons from the start.
        ui.set_min_height(32.0);
        ui.label(RichText::new("Reason").text_style(theme::label()));
        ui.add(
            egui::TextEdit::singleline(&mut review.note)
                .hint_text("Why reject? The assistant reads this.")
                .desired_width(280.0)
                .min_size(Vec2::new(0.0, 32.0)),
        );
        let reject = ui
            .add_enabled(
                !busy && !review.note.trim().is_empty(),
                theme::danger("Reject"),
            )
            .on_disabled_hover_text(if busy {
                "Waiting for the database."
            } else {
                "Write why first; the assistant reads it."
            });
        if reject.clicked() {
            request = Some(Request::RejectProposal {
                id: review.record.id.clone(),
                note: review.note.trim().to_string(),
            });
        }
        ui.separator();
        if !problems.is_empty() {
            let n = problems.len();
            theme::marked(
                ui,
                Mark::Error,
                RichText::new(format!(
                    "Fix {n} problem{} to approve",
                    if n == 1 { "" } else { "s" }
                ))
                .small()
                .family(FontFamily::Name(theme::SEMIBOLD.into())),
            );
        }
        let approve = ui
            .add_enabled(!busy && problems.is_empty(), theme::primary("Approve"))
            .on_hover_text("Saves the valve type, its parts and any new symbol to the catalogue.")
            .on_disabled_hover_text(if busy {
                "Waiting for the database."
            } else {
                "Fix the problems listed above first."
            });
        if approve.clicked()
            && let Ok(p) = edited
        {
            request = Some(Request::ApproveProposal {
                id: review.record.id.clone(),
                proposal: p.clone(),
            });
        }
    });
    request
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use eframe::egui::accesskit::{Role, Toggled};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::db::Db;

    fn shared() -> Shared {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
        Shared::new(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
    }

    fn record(status: ProposalStatus) -> ProposalRecord {
        let proposal: Proposal = serde_json::from_value(serde_json::json!({
            "valve_type": {
                "key": "vt-demo-new", "version": 0, "manufacturer": "demo",
                "series": "DEMO-10", "code": "4-3-C", "symbol": "sym-4-3-closed-centre@1",
                "mounting": "ISO 4401-05", "params": {"function": "directional"}
            },
            "parts": [{"part_number": "DEMO-10-4-3-C-24DC", "options": {"voltage": "24 V DC"}}],
            "notes": "Ordering code table on page 2."
        }))
        .unwrap();
        ProposalRecord {
            id: "p1".into(),
            status,
            proposal,
            approved: None,
            datasheet_sha256: None,
            created_by: "alice".into(),
            created_at: "2026-10-30 09:00".into(),
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
        }
    }

    type State = (Review, Shared, Option<Request>);

    fn detail_harness_sized(record: ProposalRecord, size: Vec2) -> Harness<'static, State> {
        let mut h = theme::harness(
            size,
            |ui, (r, s, out): &mut State| {
                if let Some(req) = detail(ui, r, s, false) {
                    *out = Some(req);
                }
            },
            (Review::new(record), shared(), None),
        );
        h.run();
        h
    }

    fn detail_harness(record: ProposalRecord) -> Harness<'static, State> {
        detail_harness_sized(record, Vec2::new(900.0, 700.0))
    }

    fn harness(status: ProposalStatus) -> Harness<'static, State> {
        detail_harness(record(status))
    }

    fn approve_enabled(h: &Harness<State>) -> bool {
        !h.get_by_label("Approve").accesskit_node().is_disabled()
    }

    #[test]
    fn a_reviewer_can_correct_fields_and_approve_the_edit() {
        let mut h = harness(ProposalStatus::Pending);
        assert!(h.query_by_label("Ordering code table on page 2.").is_some());
        assert!(approve_enabled(&h));
        assert!(h.query_by_label_contains("to approve").is_none());

        // Emptying the code is a problem the reviewer sees, and approval waits for a fix.
        h.state_mut().0.code.clear();
        h.run();
        assert!(
            h.query_by_label("the valve type needs a series and a code")
                .is_some()
        );
        assert!(h.query_by_label("Fix 1 problem to approve").is_some());
        assert!(!approve_enabled(&h));
        h.state_mut().0.code = "4-3-C".into();
        h.state_mut().0.params = "{not json".into();
        h.run();
        assert!(!approve_enabled(&h));
        // A new valve type must say what it does.
        h.state_mut().0.params = r#"{"max_pressure_bar": 315}"#.into();
        h.run();
        assert!(!approve_enabled(&h));
        h.state_mut().0.params = r#"{"function": "directional", "max_pressure_bar": 315}"#.into();
        h.state_mut().0.series = " DEMO-10X ".into();
        h.run();
        assert!(approve_enabled(&h));

        h.get_by_label("Approve").click();
        h.run();
        let Some(Request::ApproveProposal { id, proposal }) = &h.state().2 else {
            panic!("expected an approval")
        };
        assert_eq!(id, "p1");
        assert_eq!(proposal.valve_type.series, "DEMO-10X");
        assert_eq!(proposal.valve_type.params["max_pressure_bar"], 315);
    }

    #[test]
    fn every_problem_is_counted_and_blocks_approval() {
        let mut h = harness(ProposalStatus::Pending);
        h.state_mut().0.code.clear();
        h.state_mut().0.params = r#"{"max_pressure_bar": 315}"#.into();
        h.run();
        assert!(h.query_by_label("Fix 2 problems to approve").is_some());
        assert!(!approve_enabled(&h));
        // A click on the disabled button sends nothing.
        h.get_by_label("Approve").click();
        h.run();
        assert!(h.state().2.is_none());
    }

    #[test]
    fn the_header_shows_review_status_and_the_assistant_chip() {
        for (status, word) in [
            (ProposalStatus::Pending, "Waiting for review"),
            (ProposalStatus::Approved, "Approved"),
            (ProposalStatus::Rejected, "Rejected"),
        ] {
            let h = harness(status);
            assert!(h.query_by_label(word).is_some(), "{word}");
            assert!(h.query_by_label("Assistant draft").is_some(), "{word}");
            assert!(h.query_by_label("DEMO-10 4-3-C").is_some());
        }
    }

    #[test]
    fn rejecting_needs_a_reason_and_reviewed_proposals_are_read_only() {
        let mut h = harness(ProposalStatus::Pending);
        assert!(h.get_by_label("Reject").accesskit_node().is_disabled());
        h.state_mut().0.note = "Spool is tandem centre, see page 1".into();
        h.run();
        h.get_by_label("Reject").click();
        h.run();
        let Some(Request::RejectProposal { id, note }) = &h.state().2 else {
            panic!("expected a rejection")
        };
        assert_eq!(id, "p1");
        assert_eq!(note, "Spool is tandem centre, see page 1");

        let h = harness(ProposalStatus::Approved);
        assert!(h.query_by_label("DEMO-10 4-3-C").is_some());
        assert!(h.query_by_label("Approve").is_none());
        assert!(h.query_by_label("Reject").is_none());
    }

    #[test]
    fn updating_a_released_valve_type_shows_what_changes() {
        let mut r = record(ProposalStatus::Pending);
        r.proposal = serde_json::from_value(serde_json::json!({
            "valve_type": {
                "key": "vt-demo-4-2", "version": 1, "manufacturer": "demo",
                "series": "DEMO-06", "code": "4-2-X", "symbol": "sym-4-2-spring-return@1",
                "mounting": "ISO 4401-03"
            },
            "parts": [{"part_number": "DEMO-06-4-2-S-24DC", "options": {"voltage": "24 V DC"}},
                      {"part_number": "DEMO-06-4-2-X-12DC"}]
        }))
        .unwrap();
        let h = detail_harness(r);
        assert!(
            h.query_by_label_contains("This replaces vt-demo-4-2@1")
                .is_some()
        );
        assert!(h.query_by_label("was 4-2-S").is_some());
        // Only the code changed, so only its row is marked.
        assert_eq!(h.query_all_by_label("Changed").count(), 1);
        assert!(h.query_by_label("Parts of the new version (3)").is_some());
        for how in ["new", "replaced", "kept"] {
            assert!(h.query_by_label(how).is_some(), "{how}");
        }
    }

    type PanelState = (ProposalsPanel, Shared);

    /// The whole tab, connected to a fake database whose requests come out of the receiver.
    fn panel_harness() -> (Harness<'static, PanelState>, Receiver<Request>) {
        let (requests, sent) = channel();
        let mut shared = shared();
        shared.db = Some(Db::new(requests, channel().1));
        let h = theme::harness(
            Vec2::new(1100.0, 700.0),
            |ui, (panel, shared): &mut PanelState| panel.ui(ui, shared),
            (ProposalsPanel::default(), shared),
        );
        (h, sent)
    }

    fn listed(sent: &Receiver<Request>) -> Vec<bool> {
        sent.try_iter()
            .map(|r| match r {
                Request::ListProposals { pending_only } => pending_only,
                _ => panic!("expected a list request"),
            })
            .collect()
    }

    #[test]
    fn the_queue_switches_between_waiting_and_all() {
        let (mut h, sent) = panel_harness();
        // A spinner turns while the list loads.
        h.step();
        // The first view asks for waiting proposals only.
        assert_eq!(listed(&sent), [true]);
        h.state_mut()
            .0
            .listed(vec![record(ProposalStatus::Pending)]);
        h.step();

        h.get_by_label("All").click();
        h.step();
        assert_eq!(listed(&sent), [false]);
        let mut approved = record(ProposalStatus::Approved);
        approved.id = "p2".into();
        h.state_mut()
            .0
            .listed(vec![record(ProposalStatus::Pending), approved]);
        h.step();
        // A reviewed row shows its decision; a waiting one shows none.
        assert!(h.query_by_label("Approved").is_some());
        assert!(h.query_by_label("Waiting for review").is_none());

        // Clicking the selected segment again asks for nothing.
        h.get_by_label("All").click();
        h.step();
        assert_eq!(listed(&sent), Vec::<bool>::new());
        h.get_by_label("Waiting").click();
        h.step();
        assert_eq!(listed(&sent), [true]);
    }

    #[test]
    fn clicking_a_queue_row_opens_the_proposal() {
        let (mut h, _sent) = panel_harness();
        h.step();
        h.state_mut()
            .0
            .listed(vec![record(ProposalStatus::Pending)]);
        h.run();
        assert!(h.query_by_label("Waiting for review").is_none());
        let row = || h.get_by_role_and_label(Role::Button, "DEMO-10 4-3-C");
        assert_eq!(row().accesskit_node().toggled(), Some(Toggled::False));
        row().click();
        h.run();
        assert!(h.query_by_label("Waiting for review").is_some());
        let row = h.get_by_role_and_label(Role::Button, "DEMO-10 4-3-C");
        assert_eq!(row.accesskit_node().toggled(), Some(Toggled::True));
    }

    #[test]
    fn waiting_rows_show_their_check_and_follow_the_catalogue() {
        let (mut h, _sent) = panel_harness();
        h.step();
        let mut problems = record(ProposalStatus::Pending);
        problems.id = "p2".into();
        problems.proposal.valve_type.key = "vt-acme-1".into();
        problems.proposal.valve_type.manufacturer = "acme".into();
        problems.proposal.valve_type.params = serde_json::json!({});
        let mut update = record(ProposalStatus::Pending);
        update.id = "p3".into();
        update.proposal.valve_type = h
            .state()
            .1
            .catalogue
            .valve_types
            .iter()
            .find(|v| v.key == "vt-demo-4-2" && v.version == 1)
            .unwrap()
            .clone();
        let mut approved = record(ProposalStatus::Approved);
        approved.id = "p4".into();
        approved.proposal = problems.proposal.clone();
        h.state_mut().0.listed(vec![
            record(ProposalStatus::Pending),
            problems,
            update,
            approved,
        ]);
        h.run();
        assert!(h.query_by_label("Checks pass").is_some());
        // Unknown manufacturer and no function.
        assert!(h.query_by_label("2 problems").is_some());
        assert!(h.query_by_label("Replaces a released type").is_some());
        // A reviewed row keeps its decision, not its check.
        assert!(h.query_by_label("Approved").is_some());
        assert_eq!(h.query_all_by_label_contains("problem").count(), 1);

        // Adding the manufacturer to the catalogue leaves one problem.
        h.state_mut()
            .1
            .catalogue
            .manufacturers
            .push(hydra_core::Manufacturer {
                key: "acme".into(),
                name: "Acme".into(),
            });
        h.run();
        assert!(h.query_by_label("1 problem").is_some());
    }

    #[test]
    fn the_selected_row_follows_the_reviewers_edits() {
        let (mut h, _sent) = panel_harness();
        h.step();
        let mut draft = record(ProposalStatus::Pending);
        draft.proposal.valve_type.params = serde_json::json!({});
        h.state_mut().0.listed(vec![draft]);
        h.run();
        h.get_by_role_and_label(Role::Button, "DEMO-10 4-3-C")
            .click();
        h.run();
        assert!(h.query_by_label("1 problem").is_some());
        // The reviewer says what the valve does: the row agrees with the action bar.
        h.state_mut().0.review.as_mut().unwrap().params = r#"{"function": "relief"}"#.into();
        h.run();
        assert!(h.query_by_label("1 problem").is_none());
        assert!(h.query_by_label("Checks pass").is_some());
        assert!(!h.get_by_label("Approve").accesskit_node().is_disabled());
    }

    #[test]
    fn no_symbol_is_said_at_the_right_of_the_caption() {
        let mut r = record(ProposalStatus::Pending);
        r.proposal.valve_type.symbol = None;
        let h = detail_harness(r);
        let right = |label: &str| {
            let node = h.get_by_label(label);
            node.accesskit_node().raw_bounds().unwrap().x1
        };
        // The caption strip is 360 px wide; its right side holds the symbol's ref or this.
        assert!(right("No symbol") - right("How Hydra will draw it") > 150.0);
    }

    #[test]
    fn a_narrow_detail_still_draws() {
        // The queue dragged wide or the dock split leaves the detail a sliver.
        for width in [250.0, 40.0] {
            let h = detail_harness_sized(record(ProposalStatus::Pending), Vec2::new(width, 700.0));
            assert!(h.query_by_label("Approve").is_some());
            assert!(h.query_by_label("Ordering code table on page 2.").is_some());
        }
    }
}

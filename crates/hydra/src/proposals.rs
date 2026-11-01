//! Proposals tab: the review queue for catalogue entries an AI assistant drafted from a
//! datasheet through `hydra-mcp`. The engineer checks the rendered symbol against the
//! datasheet, may correct the valve type's fields, then approves or rejects with a reason.
//! Only approval writes catalogue rows.

use eframe::egui::{self, Vec2};
use hydra_core::{Catalogue, CatalogueRef, Proposal, SymbolDef};
use hydra_db::{ProposalRecord, ProposalStatus};
use hydra_symbols::render_symbol;

use crate::catalogue::Shared;
use crate::db::Request;
use crate::schematic::draw_fitted;

#[derive(Default)]
pub struct ProposalsPanel {
    /// `None` until the first list arrives.
    list: Option<Vec<ProposalRecord>>,
    show_reviewed: bool,
    review: Option<Review>,
    /// A list, approve or reject request is waiting for its reply.
    busy: bool,
    error: Option<String>,
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
            "{}\0{}\0{}\0{}\0{} {} {} {}",
            self.series,
            self.code,
            self.mounting,
            self.params,
            catalogue.manufacturers.len(),
            catalogue.symbol_defs.len(),
            catalogue.valve_types.len(),
            catalogue.parts.len()
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
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Refresh"))
                .clicked()
            {
                self.refresh(shared);
            }
            let toggle = ui.add_enabled(
                !self.busy,
                egui::Checkbox::new(&mut self.show_reviewed, "Show reviewed"),
            );
            if toggle.changed() {
                self.refresh(shared);
            }
            if self.busy {
                ui.spinner();
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }

        egui::Panel::left("proposal-list")
            .default_size(300.0)
            .min_size(220.0)
            .resizable(true)
            .show(ui, |ui| self.list_ui(ui, &shared.catalogue));
        egui::ScrollArea::vertical().show(ui, |ui| match self.review.as_mut() {
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
                ui.weak(
                    "Select a proposal. An AI assistant connected through hydra-mcp drafts them \
                     from the datasheets attached to valve types.",
                );
            }
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, catalogue: &Catalogue) {
        let Some(list) = &self.list else {
            return;
        };
        if list.is_empty() {
            ui.weak(if self.show_reviewed {
                "No proposals yet."
            } else {
                "No proposals waiting for review."
            });
        }
        let mut picked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for p in list {
                let v = &p.proposal.valve_type;
                let manufacturer = catalogue
                    .manufacturers
                    .iter()
                    .find(|m| m.key == v.manufacturer)
                    .map_or(v.manufacturer.as_str(), |m| m.name.as_str());
                let status = match p.status {
                    ProposalStatus::Pending => "waiting",
                    ProposalStatus::Approved => "approved",
                    ProposalStatus::Rejected => "rejected",
                };
                let text = format!(
                    "{} {} · {manufacturer}\n{} · {} · {} · {status}",
                    v.series,
                    v.code,
                    p.proposal.target(),
                    p.created_by,
                    p.created_at
                );
                let selected = self.review.as_ref().is_some_and(|r| r.record.id == p.id);
                if ui.selectable_label(selected, text).clicked() {
                    picked = Some(p.clone());
                }
                ui.add_space(2.0);
            }
        });
        if let Some(p) = picked {
            self.review = Some(Review::new(p));
        }
    }
}

/// One proposal: symbol, fields, parts and what's wrong; returns a review request to send.
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

    let symbol: Option<&SymbolDef> = match (&proposal.new_symbol, &v.symbol) {
        (Some(new), _) => Some(&new.definition),
        (None, Some(s)) => catalogue.symbol_def(s),
        (None, None) => None,
    };
    let mut request = None;
    ui.horizontal_top(|ui| {
        let side = Vec2::new(320.0, 220.0);
        match symbol {
            Some(def) => draw_fitted(ui, side, &render_symbol(def).primitives),
            None => {
                ui.allocate_exact_size(side, egui::Sense::hover());
            }
        }
        ui.vertical(|ui| {
            ui.heading(format!("{} {}", v.series, v.code));
            ui.monospace(format!("{} → {}", v.key, proposal.target()));
            match (&proposal.new_symbol, &v.symbol) {
                (Some(new), _) => ui.label(format!("New symbol {}", new.reference())),
                (None, Some(s)) => ui.label(format!("Symbol {s}")),
                (None, None) => ui.colored_label(ui.visuals().error_fg_color, "No symbol"),
            };
            ui.weak(format!(
                "Drafted by {} on {}",
                review.record.created_by, review.record.created_at
            ));
            if let Some(by) = &review.record.reviewed_by {
                let when = review.record.reviewed_at.as_deref().unwrap_or("");
                ui.weak(format!("Reviewed by {by} on {when}"));
            }
            if let Some(note) = &review.record.review_note {
                ui.label(format!("Reason: {note}"));
            }
            match (&review.record.datasheet_sha256, &shared.db) {
                (Some(sha256), Some(_)) => {
                    if ui.button("Open datasheet").clicked() {
                        request = Some(Request::OpenAsset {
                            sha256: sha256.clone(),
                            extension: "pdf",
                        });
                    }
                }
                _ => {
                    ui.weak("No datasheet attached.");
                }
            }
        });
    });

    ui.separator();
    if pending && let Some(b) = base.as_ref().filter(|b| b.symbol.is_some()) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "This replaces {}, which is not a draft and may be in use. Check every change.",
                b.reference()
            ),
        );
    }
    let was = |ui: &mut egui::Ui, old: Option<String>, new: &str| {
        if let Some(old) = old.filter(|o| o != new) {
            ui.weak(format!(
                "was {}",
                if old.is_empty() { "empty" } else { &old }
            ));
        }
    };
    egui::Grid::new("proposal-fields")
        .num_columns(3)
        .show(ui, |ui| {
            ui.label("Manufacturer");
            let manufacturer = catalogue
                .manufacturers
                .iter()
                .find(|m| m.key == v.manufacturer);
            ui.label(manufacturer.map_or(v.manufacturer.as_str(), |m| m.name.as_str()));
            ui.end_row();
            let b = base.as_ref();
            for (label, value, old) in [
                ("Series", &mut review.series, b.map(|b| b.series.clone())),
                ("Code", &mut review.code, b.map(|b| b.code.clone())),
                (
                    "Mounting",
                    &mut review.mounting,
                    b.map(|b| b.mounting.clone().unwrap_or_default()),
                ),
            ] {
                ui.label(label);
                ui.add_enabled(
                    pending,
                    egui::TextEdit::singleline(value).desired_width(260.0),
                );
                was(ui, old, value);
                ui.end_row();
            }
            ui.label("Symbol");
            let symbol = v.symbol.as_ref().map(|s| s.to_string()).unwrap_or_default();
            ui.monospace(&symbol);
            was(
                ui,
                b.map(|b| b.symbol.as_ref().map(|s| s.to_string()).unwrap_or_default()),
                &symbol,
            );
            ui.end_row();
            ui.label("Parameters");
            ui.add_enabled(
                pending,
                egui::TextEdit::multiline(&mut review.params)
                    .code_editor()
                    .desired_rows(3)
                    .desired_width(260.0),
            );
            let params = |p: &serde_json::Value| {
                if p.is_null() {
                    String::new()
                } else {
                    serde_json::to_string(p).unwrap_or_default()
                }
            };
            was(ui, b.map(|b| params(&b.params)), &params(&v.params));
            ui.end_row();
        });

    let checked = review.checked(catalogue);
    if !checked.parts.is_empty() {
        ui.label(format!(
            "Parts of the new version ({})",
            checked.parts.len()
        ));
        for (number, options, how) in &checked.parts {
            ui.horizontal(|ui| {
                ui.monospace(number);
                ui.weak(*how);
                ui.weak(options);
            });
        }
    }
    if !proposal.notes.is_empty() {
        ui.label("Assistant's notes");
        ui.weak(&proposal.notes);
    }

    if !pending {
        return request;
    }
    ui.separator();
    // Checked against the catalogue as it is now: someone may have saved since.
    let problems = review.checked(catalogue).problems.clone();
    for p in &problems {
        ui.colored_label(ui.visuals().error_fg_color, p);
    }
    ui.horizontal(|ui| {
        let approve = ui
            .add_enabled(!busy && problems.is_empty(), egui::Button::new("Approve"))
            .on_hover_text("Saves the valve type, its parts and any new symbol to the catalogue.");
        if approve.clicked()
            && let Ok(p) = &edited
        {
            request = Some(Request::ApproveProposal {
                id: review.record.id.clone(),
                proposal: p.clone(),
            });
        }
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut review.note)
                .hint_text("Why reject? The assistant reads this.")
                .desired_width(280.0),
        );
        let reject = ui.add_enabled(
            !busy && !review.note.trim().is_empty(),
            egui::Button::new("Reject"),
        );
        if reject.clicked() {
            request = Some(Request::RejectProposal {
                id: review.record.id.clone(),
                note: review.note.trim().to_string(),
            });
        }
    });
    request
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

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

    fn harness(status: ProposalStatus) -> Harness<'static, State> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(900.0, 700.0))
            .build_ui_state(
                |ui, (r, s, out): &mut State| {
                    if let Some(req) = detail(ui, r, s, false) {
                        *out = Some(req);
                    }
                },
                (Review::new(record(status)), shared(), None),
            );
        h.run();
        h
    }

    fn approve_enabled(h: &Harness<State>) -> bool {
        !h.get_by_label("Approve").accesskit_node().is_disabled()
    }

    #[test]
    fn a_reviewer_can_correct_fields_and_approve_the_edit() {
        let mut h = harness(ProposalStatus::Pending);
        assert!(h.query_by_label("Ordering code table on page 2.").is_some());
        assert!(approve_enabled(&h));

        // Emptying the code is a problem the reviewer sees, and approval waits for a fix.
        h.state_mut().0.code.clear();
        h.run();
        assert!(
            h.query_by_label("the valve type needs a series and a code")
                .is_some()
        );
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
    fn rejecting_needs_a_reason_and_reviewed_proposals_are_read_only() {
        let mut h = harness(ProposalStatus::Pending);
        assert!(h.get_by_label("Reject").accesskit_node().is_disabled());
        h.state_mut().0.note = "Spool is tandem centre, see page 1".into();
        h.run();
        h.get_by_label("Reject").click();
        h.run();
        let Some(Request::RejectProposal { note, .. }) = &h.state().2 else {
            panic!("expected a rejection")
        };
        assert_eq!(note, "Spool is tandem centre, see page 1");

        let h = harness(ProposalStatus::Approved);
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
        let mut h = Harness::builder()
            .with_size(Vec2::new(900.0, 700.0))
            .build_ui_state(
                |ui, (r, s, out): &mut State| {
                    if let Some(req) = detail(ui, r, s, false) {
                        *out = Some(req);
                    }
                },
                (Review::new(r), shared(), None),
            );
        h.run();
        assert!(
            h.query_by_label_contains("This replaces vt-demo-4-2@1")
                .is_some()
        );
        assert!(h.query_by_label("was 4-2-S").is_some());
        assert!(h.query_by_label("Parts of the new version (3)").is_some());
        for how in ["new", "replaced", "kept"] {
            assert!(h.query_by_label(how).is_some(), "{how}");
        }
    }
}

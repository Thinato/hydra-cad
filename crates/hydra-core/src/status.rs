//! How each valve stands against the catalogue: missing, part not listed or a stand-in, which
//! catalogue valves could stand in, and whether the requested valve has become available.

use std::collections::BTreeMap;
use std::fmt;

use crate::{
    Catalogue, CatalogueRef, Circuit, Command, Component, Issue, Ratings, SymbolDef, Valve,
    ValveFunction,
};

/// A valve's standing, derived from what the drawing requested and what the circuit uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValveStatus {
    /// Uses the requested part, or nothing was requested.
    Normal,
    /// Amber: the valve type is in the catalogue, but not the exact requested part.
    PartNotListed,
    /// Red: nothing in the catalogue is used for it.
    Missing,
    /// Blue: an engineer chose a temporary replacement.
    StandIn,
}

impl ValveStatus {
    /// The status in words for drawings and the bill of materials; empty when normal.
    pub fn words(self) -> &'static str {
        match self {
            ValveStatus::Normal => "",
            ValveStatus::PartNotListed => "PART NOT LISTED",
            ValveStatus::Missing => "MISSING",
            ValveStatus::StandIn => "STAND-IN",
        }
    }
}

/// The requested valve can now be used: this valve type, and this part when one is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub valve_type: CatalogueRef,
    pub part: Option<String>,
}

impl Component {
    /// Status of a valve; `None` for other components, and for a valve whose catalogue type is
    /// gone without a request to compare with (that stays an [`Issue::UnknownValveType`]).
    pub fn valve_status(&self, catalogue: &Catalogue) -> Option<ValveStatus> {
        let v = self.valve()?;
        let Some(t) = &v.valve_type else {
            return Some(ValveStatus::Missing);
        };
        if catalogue.valve_type(t).is_none() {
            return v.requested.is_some().then_some(ValveStatus::Missing);
        }
        if v.stand_in {
            return Some(ValveStatus::StandIn);
        }
        let Some(code) = requested_code(v) else {
            return Some(ValveStatus::Normal);
        };
        let listed = catalogue.parts_of(t).any(|p| p.part_number.trim() == code);
        Some(
            if listed && v.part.as_deref().map(str::trim) == Some(code) {
                ValveStatus::Normal
            } else {
                ValveStatus::PartNotListed
            },
        )
    }

    /// For a valve that isn't normal: where the requested valve is now in the catalogue.
    ///
    /// Either a part numbered exactly as requested is on the latest version of a valve type, or
    /// the requested key now exists while the valve is missing or stands in.
    pub fn requested_available(&self, catalogue: &Catalogue) -> Option<Available> {
        let status = self.valve_status(catalogue)?;
        if status == ValveStatus::Normal {
            return None;
        }
        let v = self.valve()?;
        let latest = catalogue.latest_valve_types();
        if let Some(code) = requested_code(v) {
            for t in latest.values() {
                let r = t.reference();
                if catalogue.parts_of(&r).any(|p| p.part_number.trim() == code) {
                    return Some(Available {
                        valve_type: r,
                        part: Some(code.to_string()),
                    });
                }
            }
        }
        let key = v.requested.as_ref()?.key.as_deref()?;
        let t = latest.get(key)?;
        matches!(status, ValveStatus::Missing | ValveStatus::StandIn).then(|| Available {
            valve_type: t.reference(),
            part: None,
        })
    }
}

fn requested_code(v: &Valve) -> Option<&str> {
    v.requested
        .as_ref()
        .map(|r| r.code.trim())
        .filter(|c| !c.is_empty())
}

/// A catalogue valve that could replace a valve, and how its ports would connect.
#[derive(Debug, Clone, PartialEq)]
pub struct StandIn {
    pub valve_type: CatalogueRef,
    pub part: Option<String>,
    /// It is exactly the requested part.
    pub exact: bool,
    /// `(old, new)` port names, as [`Command::Use`] takes them.
    pub port_map: Vec<(String, String)>,
    /// Its ports differ, so some connections need checking or redrawing.
    pub rewire: bool,
}

/// Why nothing in the catalogue can stand in.
#[derive(Debug, Clone, PartialEq)]
pub struct NoStandIn {
    pub function: Option<ValveFunction>,
    pub mounting: Option<String>,
}

impl fmt::Display for NoStandIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let function = self.function.map_or("a known function".to_string(), |x| {
            x.name().replace('_', " ")
        });
        let mounting = self.mounting.as_deref().unwrap_or("no mounting");
        write!(f, "no compatible stand-in: needs {function}, {mounting}")
    }
}

/// What a replacement has to do, from the request with the current valve type filling gaps.
struct Need<'a> {
    function: Option<ValveFunction>,
    mounting: Option<String>,
    setting_bar: Option<f64>,
    max_pressure_bar: Option<f64>,
    max_flow_l_min: Option<f64>,
    /// The symbol a directional valve or flow control must match.
    symbol: Option<&'a SymbolDef>,
}

impl<'a> Need<'a> {
    fn of(v: &'a Valve, catalogue: &'a Catalogue) -> Need<'a> {
        let current = v
            .valve_type
            .as_ref()
            .filter(|t| catalogue.valve_type(t).is_some());
        let rq = v.requested.as_ref();
        let rated = current.map(|t| catalogue.ratings_of(t, v.part.as_deref()));
        let part_setting = current.and_then(|t| {
            let part = v.part.as_deref()?;
            let p = catalogue.parts_of(t).find(|p| p.part_number == part)?;
            p.options.get("setting_bar")?.as_f64()
        });
        Need {
            function: rq
                .and_then(|r| r.function)
                .or_else(|| catalogue.function_of(current?)),
            mounting: rq.and_then(|r| r.mounting.clone()).or_else(|| {
                current
                    .and_then(|t| catalogue.valve_type(t))
                    .and_then(|t| t.mounting.clone())
            }),
            setting_bar: rq.and_then(|r| r.setting_bar).or(part_setting),
            max_pressure_bar: rq
                .and_then(|r| r.max_pressure_bar)
                .or(rated.and_then(|r| r.max_pressure_bar)),
            max_flow_l_min: rq
                .and_then(|r| r.max_flow_l_min)
                .or(rated.and_then(|r| r.max_flow_l_min)),
            symbol: match current {
                Some(t) => catalogue.symbol_of(t),
                None => v.display.as_ref(),
            },
        }
    }

    /// `None` when `r` can't do the job; otherwise how far it overshoots and how many of the
    /// needed ratings it leaves unknown, both smaller for a better match.
    fn fit(&self, function: ValveFunction, r: &Ratings) -> Option<(f64, usize)> {
        let (mut surplus, mut unknown) = (0.0, 0);
        let mut at_least = |need: Option<f64>, have: Option<f64>| match (need, have) {
            (Some(n), Some(h)) if h < n => false,
            (Some(n), Some(h)) => {
                surplus += (h - n) / n.max(f64::EPSILON);
                true
            }
            (Some(_), None) => {
                unknown += 1;
                true
            }
            (None, _) => true,
        };
        if !at_least(self.max_flow_l_min, r.max_flow_l_min) {
            return None;
        }
        if function.is_pressure_setting() {
            match (self.setting_bar, r.setting_range_bar) {
                (Some(s), Some([lo, hi])) if s < lo || s > hi => return None,
                (Some(_), None) => unknown += 1,
                _ => {}
            }
        } else if !at_least(self.max_pressure_bar, r.max_pressure_bar) {
            return None;
        }
        Some((surplus, unknown))
    }
}

/// Catalogue valves that could replace valve `id`, best first: the exact requested part (always
/// offered), then the same symbol, then known ratings, then the closest ratings.
///
/// Candidates do the same job, fit the same mounting or cavity, and meet the needed ratings
/// where both sides know them. A directional valve or flow control must also have the same
/// symbol, so a tandem-centre spool never replaces a closed-centre one.
pub fn stand_ins(
    circuit: &Circuit,
    id: &str,
    catalogue: &Catalogue,
) -> Result<Vec<StandIn>, NoStandIn> {
    let nothing = || NoStandIn {
        function: None,
        mounting: None,
    };
    let c = circuit.component(id).ok_or_else(nothing)?;
    let v = c.valve().ok_or_else(nothing)?;
    let need = Need::of(v, catalogue);
    let old_ports = c.ports(catalogue).unwrap_or_default();
    let offer = |t: &CatalogueRef, part: Option<String>, exact: bool| {
        let new_ports = catalogue
            .symbol_of(t)
            .map(SymbolDef::ports)
            .unwrap_or_default();
        let port_map = Command::default_port_map(&old_ports, &new_ports);
        let rewire = port_map.len() != old_ports.len()
            || port_map.iter().any(|(o, n)| o != n)
            || new_ports.len() != old_ports.len();
        StandIn {
            valve_type: t.clone(),
            part,
            exact,
            port_map,
            rewire,
        }
    };
    let used = (v.valve_type.as_ref(), v.part.as_deref());
    let mut found: Vec<(Rank, StandIn)> = Vec::new();

    let latest = catalogue.latest_valve_types();
    if let Some(code) = requested_code(v) {
        for t in latest.values() {
            let r = t.reference();
            if catalogue.parts_of(&r).any(|p| p.part_number.trim() == code)
                && used != (Some(&r), Some(code))
            {
                found.push((
                    (0, 0, 0, 0.0, r.key.clone(), code.to_string()),
                    offer(&r, Some(code.to_string()), true),
                ));
            }
        }
    }

    if let Some(function) = need.function.filter(|f| *f != ValveFunction::Other) {
        let strict_symbol = matches!(
            function,
            ValveFunction::Directional | ValveFunction::FlowControl
        );
        for t in latest.values() {
            let r = t.reference();
            if catalogue.function_of(&r) != Some(function) || t.mounting != need.mounting {
                continue;
            }
            let symbol = catalogue.symbol_of(&r);
            let same_symbol = need.symbol.is_some() && symbol == need.symbol;
            if strict_symbol && need.symbol.is_some() && !same_symbol {
                continue;
            }
            // Parts with ratings of their own are separate candidates.
            let parts: Vec<_> = catalogue.parts_of(&r).collect();
            let units: Vec<Option<String>> = if parts.iter().any(|p| has_ratings(&p.options)) {
                parts.iter().map(|p| Some(p.part_number.clone())).collect()
            } else if let [only] = parts.as_slice() {
                vec![Some(only.part_number.clone())]
            } else {
                vec![None]
            };
            for part in units {
                if used == (Some(&r), part.as_deref())
                    || found
                        .iter()
                        .any(|(_, s)| s.valve_type == r && s.part == part)
                {
                    continue;
                }
                let ratings = catalogue.ratings_of(&r, part.as_deref());
                let Some((surplus, unknown)) = need.fit(function, &ratings) else {
                    continue;
                };
                let rank = (
                    1,
                    u8::from(!same_symbol),
                    unknown,
                    surplus,
                    r.key.clone(),
                    part.clone().unwrap_or_default(),
                );
                found.push((rank, offer(&r, part, false)));
            }
        }
    }

    if found.is_empty() {
        return Err(NoStandIn {
            function: need.function,
            mounting: need.mounting,
        });
    }
    found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(found.into_iter().map(|(_, s)| s).collect())
}

/// Sort key: exact part first, then same symbol, unknown ratings, surplus, key, part.
type Rank = (u8, u8, usize, f64, String, String);

fn has_ratings(options: &serde_json::Value) -> bool {
    Ratings::KEYS.iter().any(|k| options.get(k).is_some())
}

/// Every valve's status, and the warnings they raise, computed once per edit and catalogue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CircuitStatus {
    pub valves: BTreeMap<String, (ValveStatus, Option<Available>)>,
    /// Warnings only: none of these blocks saving or export.
    pub issues: Vec<Issue>,
}

impl CircuitStatus {
    pub fn compute(circuit: &Circuit, catalogue: &Catalogue) -> CircuitStatus {
        let mut out = CircuitStatus::default();
        for c in &circuit.components {
            let Some(status) = c.valve_status(catalogue) else {
                continue;
            };
            let available = c.requested_available(catalogue);
            let code = c
                .valve()
                .and_then(requested_code)
                .unwrap_or_default()
                .to_string();
            let component = c.id.clone();
            match status {
                ValveStatus::Normal => {}
                ValveStatus::Missing => out.issues.push(Issue::ValveMissing {
                    component: component.clone(),
                    code: code.clone(),
                }),
                ValveStatus::PartNotListed => out.issues.push(Issue::PartNotListed {
                    component: component.clone(),
                    code: code.clone(),
                }),
                ValveStatus::StandIn => out.issues.push(Issue::StandIn {
                    component: component.clone(),
                    code: code.clone(),
                }),
            }
            if let Some(a) = &available {
                out.issues.push(Issue::RequestedNowAvailable {
                    component: component.clone(),
                    code,
                    valve_type: a.valve_type.clone(),
                });
            }
            out.valves.insert(component, (status, available));
        }
        out
    }

    pub fn status(&self, id: &str) -> Option<ValveStatus> {
        self.valves.get(id).map(|(s, _)| *s)
    }
}

//! Per-frame meters: audio features and each modulation target's current value.

use eframe::egui;
use reactive_vcr::effect::EffectSettings;
use reactive_vcr::mapping::Source;
use reactive_vcr::params::{ParamCatalog, get_param};

use crate::analysis::Analysis;

const BAR: egui::Vec2 = egui::vec2(96.0, 8.0);

struct Meter {
    label: String,
    fraction: f32,
    value: String,
    color: egui::Color32,
    hover: Option<String>,
}

/// Draws feature meters for `frame` and target meters read from `effect`.
pub fn show(
    ui: &mut egui::Ui,
    catalog: &ParamCatalog,
    analysis: &Analysis,
    frame: usize,
    effect: &EffectSettings,
) {
    let features = &analysis.features;
    let level = ui.visuals().selection.bg_fill;
    let hit = ui.visuals().warn_fg_color;
    let feature = |label: &str, source: Source, color| {
        let value = features.value(source, frame);
        Meter {
            label: label.to_string(),
            fraction: value,
            value: format!("{value:.2}"),
            color,
            hover: None,
        }
    };
    let mut audio = vec![feature("rms", Source::Rms, level)];
    audio.extend(
        analysis
            .bands
            .iter()
            .enumerate()
            .map(|(index, name)| feature(name, Source::Band(index), level)),
    );
    let mut onset = feature("onset", Source::Onset, hit);
    onset.value = if onset.fraction > 0.0 { "hit" } else { "—" }.into();
    audio.push(onset);
    rows(ui, &audio);

    let targets: Vec<Meter> = analysis
        .targets
        .iter()
        .map(|target| {
            let param = catalog.param(target.param);
            let value = get_param(effect, param);
            let (lo, hi) = (f64::from(target.range.0), f64::from(target.range.1));
            let (fraction, text) = if param.kind.is_bool() {
                (value, if value >= 0.5 { "on" } else { "off" }.to_string())
            } else if (hi - lo).abs() > f64::EPSILON {
                ((value - lo) / (hi - lo), format!("{value:.3}"))
            } else {
                (f64::from(value >= lo), format!("{value:.3}"))
            };
            Meter {
                label: crate::editor::param_label(catalog, target.param),
                fraction: fraction as f32,
                value: text,
                color: hit,
                hover: Some(format!(
                    "{} ← {}\nrange {}..{}",
                    param.name, target.source, target.range.0, target.range.1
                )),
            }
        })
        .collect();
    if !targets.is_empty() {
        ui.add_space(4.0);
        rows(ui, &targets);
    }
}

/// Lays meters out in as many rows as the width needs.
fn rows(ui: &mut egui::Ui, meters: &[Meter]) {
    let step = BAR.x + ui.spacing().item_spacing.x;
    let columns = ((ui.available_width() + ui.spacing().item_spacing.x) / step).floor() as usize;
    for row in meters.chunks(columns.max(1)) {
        ui.horizontal(|ui| {
            for m in row {
                let response = meter(ui, m);
                if let Some(hover) = &m.hover {
                    response.on_hover_text(hover);
                }
            }
        });
    }
}

/// A labelled bar filled to `fraction` (clamped to 0..1).
fn meter(ui: &mut egui::Ui, m: &Meter) -> egui::Response {
    ui.vertical(|ui| {
        ui.set_width(BAR.x);
        ui.add(egui::Label::new(egui::RichText::new(&m.label).small()).truncate());
        let (rect, response) = ui.allocate_exact_size(BAR, egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        let mut fill = rect;
        fill.set_width(rect.width() * m.fraction.clamp(0.0, 1.0));
        painter.rect_filled(fill, 2.0, m.color);
        ui.label(egui::RichText::new(&m.value).small().weak());
        response
    })
    .inner
}

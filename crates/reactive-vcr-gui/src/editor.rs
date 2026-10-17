//! The in-memory mapping and the editor panel that shapes it.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use eframe::egui;
use reactive_vcr::curve::Curve;
use reactive_vcr::effect::{BendOrder, EffectSettings};
use reactive_vcr::mapping::{
    AudioSection, BaseSection, BenderSection, CompiledMapping, MappingFile, ModulationSection,
    compile,
};
use reactive_vcr::params::{
    BENDER_PREFIX, Param, ParamCatalog, ParamKind, ParamSection, get_param,
};

/// The mapping being edited, its file, and its last successful compile.
pub struct MappingDoc {
    pub file: MappingFile,
    /// Absolute path of the mapping file; `None` until saved or opened.
    pub path: Option<PathBuf>,
    /// The file contents as last loaded or saved, for the unsaved-changes marker.
    saved: MappingFile,
    /// Last mapping that compiled; kept while edits have errors.
    pub compiled: Option<CompiledMapping>,
    /// Problems with the current edits.
    pub errors: Option<String>,
}

impl MappingDoc {
    pub fn new(file: MappingFile, path: Option<PathBuf>, saved: MappingFile) -> Self {
        Self {
            file,
            path,
            saved,
            compiled: None,
            errors: None,
        }
    }

    pub fn empty() -> Self {
        Self::new(MappingFile::default(), None, MappingFile::default())
    }

    pub fn open(path: &Path) -> Result<Self> {
        let path = absolute(path);
        let file = read_mapping(&path)?;
        Ok(Self::new(file.clone(), Some(path), file))
    }

    /// Restores persisted edits of the mapping at `path` (if any).
    pub fn restore(text: &str, path: Option<PathBuf>) -> Result<Self> {
        let file = MappingFile::parse(text).context("restoring the last mapping")?;
        let saved = path
            .as_deref()
            .and_then(|p| read_mapping(p).ok())
            .unwrap_or_default();
        Ok(Self::new(file, path, saved))
    }

    pub fn is_dirty(&self) -> bool {
        self.file != self.saved
    }

    pub fn title(&self) -> String {
        let name = self.path.as_deref().and_then(Path::file_name).map_or_else(
            || "untitled mapping".into(),
            |n| n.to_string_lossy().into_owned(),
        );
        if self.is_dirty() {
            format!("{name}*")
        } else {
            name
        }
    }

    /// Directory that relative preset paths resolve against.
    fn base_dir(&self) -> PathBuf {
        self.path
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| absolute(Path::new(".")), Path::to_path_buf)
    }

    /// Drops empty sections and recompiles. Returns true when the mapping compiled.
    pub fn recompile(&mut self, catalog: &ParamCatalog) -> bool {
        tidy(&mut self.file);
        match compile(self.file.clone(), &self.base_dir(), catalog) {
            Ok(compiled) => {
                self.compiled = Some(compiled);
                self.errors = None;
                true
            }
            Err(e) => {
                self.errors = Some(format!("{e:#}"));
                false
            }
        }
    }

    /// Path to store for a preset chosen in a dialog: relative to a saved mapping when
    /// possible, absolute otherwise.
    fn preset_reference(&self, chosen: &Path) -> PathBuf {
        let chosen = absolute(chosen);
        match &self.path {
            Some(_) => relative_to(&chosen, &self.base_dir()).unwrap_or(chosen),
            None => chosen,
        }
    }

    /// Writes the mapping to `path`, rewriting preset paths relative to its directory.
    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        let path = absolute(path);
        let dir = path
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let old_dir = self.base_dir();
        let mut file = self.file.clone();
        let presets = [
            file.base.as_mut().and_then(|b| b.preset.as_mut()),
            file.bender.as_mut().and_then(|b| b.preset.as_mut()),
        ];
        for preset in presets.into_iter().flatten() {
            let resolved = absolute(&old_dir.join(&*preset));
            *preset = relative_to(&resolved, &dir).unwrap_or(resolved);
        }
        let text = file.to_toml()?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        self.file = file.clone();
        self.saved = file;
        self.path = Some(path);
        Ok(())
    }
}

fn read_mapping(path: &Path) -> Result<MappingFile> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    MappingFile::parse(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Removes sections left empty by edits so saved TOML stays minimal.
fn tidy(file: &mut MappingFile) {
    if file.base.as_ref() == Some(&BaseSection::default()) {
        file.base = None;
    }
    if file.bender.as_ref() == Some(&BenderSection::default()) {
        file.bender = None;
    }
    if file.audio.as_ref() == Some(&AudioSection::default()) {
        file.audio = None;
    }
}

/// `path` made absolute without touching the filesystem beyond the current directory.
pub fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// `path` relative to directory `base`, when both are absolute and share more than the root.
fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || !base.is_absolute() {
        return None;
    }
    fn normal(p: &Path) -> Vec<Component<'_>> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect()
    }
    let (path_parts, base_parts) = (normal(path), normal(base));
    let common = path_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let shares_more_than_root = path_parts[..common]
        .iter()
        .any(|c| matches!(c, Component::Normal(_)));
    if !shares_more_than_root {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in common..base_parts.len() {
        relative.push("..");
    }
    for part in &path_parts[common..] {
        relative.push(part);
    }
    Some(relative)
}

/// Draws the mapping editor. Returns true when the mapping changed.
pub fn show(ui: &mut egui::Ui, doc: &mut MappingDoc, catalog: &ParamCatalog) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.heading("Mapping");
        ui.label(doc.title()).on_hover_text(
            doc.path
                .as_deref()
                .map_or_else(|| "Not saved yet".into(), |p| p.display().to_string()),
        );
    });
    if let Some(errors) = &doc.errors {
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0, ui.visuals().error_fg_color))
            .show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, errors);
                ui.weak("The preview keeps the last valid mapping.");
            });
    }
    ui.spacing_mut().slider_width = 130.0;

    egui::CollapsingHeader::new("Audio analysis")
        .default_open(true)
        .show(ui, |ui| changed |= audio_section(ui, &mut doc.file));

    let count = doc.file.modulate.len();
    egui::CollapsingHeader::new(format!("Modulations ({count})"))
        .id_salt("modulations")
        .default_open(true)
        .show(ui, |ui| changed |= modulations(ui, &mut doc.file, catalog));

    egui::CollapsingHeader::new("Base look")
        .default_open(true)
        .show(ui, |ui| changed |= base_look(ui, doc, catalog));
    changed
}

fn audio_section(ui: &mut egui::Ui, file: &mut MappingFile) -> bool {
    let original = file.audio.clone().unwrap_or_default();
    let mut audio = original.clone();
    let mut renames = Vec::new();
    let mut removed = None;

    ui.label("Bands");
    egui::Grid::new("bands")
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            for (name, edges) in &original.bands {
                let id = ui.make_persistent_id(("band-name", name));
                let mut text = ui
                    .data(|d| d.get_temp::<String>(id))
                    .unwrap_or_else(|| name.clone());
                let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(64.0));
                if response.changed() {
                    ui.data_mut(|d| d.insert_temp(id, text.clone()));
                }
                if response.lost_focus() {
                    ui.data_mut(|d| d.remove::<String>(id));
                    let new = text.trim();
                    if !new.is_empty() && new != name && !original.bands.contains_key(new) {
                        renames.push((name.clone(), new.to_string()));
                    }
                }
                let mut range = *edges;
                ui.add(
                    egui::DragValue::new(&mut range[0])
                        .range(0.0..=24_000.0)
                        .speed(5.0)
                        .suffix(" Hz"),
                )
                .on_hover_text("Low edge");
                ui.add(
                    egui::DragValue::new(&mut range[1])
                        .range(0.0..=24_000.0)
                        .speed(5.0)
                        .suffix(" Hz"),
                )
                .on_hover_text("High edge");
                if range != *edges {
                    audio.bands.insert(name.clone(), range);
                }
                if ui.small_button("×").on_hover_text("Remove band").clicked() {
                    removed = Some(name.clone());
                }
                ui.end_row();
            }
        });
    if ui.button("Add band").clicked() {
        let name = (1..)
            .map(|i| format!("band{i}"))
            .find(|n| !audio.bands.contains_key(n))
            .expect("unbounded names");
        audio.bands.insert(name, [200.0, 2000.0]);
    }
    if let Some(name) = removed {
        audio.bands.remove(&name);
    }
    for (old, new) in &renames {
        if let Some(edges) = audio.bands.remove(old) {
            audio.bands.insert(new.clone(), edges);
        }
        let (old_source, new_source) = (format!("band:{old}"), format!("band:{new}"));
        for m in &mut file.modulate {
            if m.source == old_source {
                m.source = new_source.clone();
            }
        }
    }

    ui.horizontal(|ui| {
        ui.label("Normalize");
        let current = audio.normalize.clone().unwrap_or_else(|| "peak".into());
        let percentile = current
            .strip_prefix("percentile:")
            .and_then(|p| p.trim().parse::<f32>().ok());
        egui::ComboBox::from_id_salt("normalize")
            .selected_text(if percentile.is_some() {
                "percentile"
            } else {
                "peak"
            })
            .show_ui(ui, |ui| {
                if ui.selectable_label(percentile.is_none(), "peak").clicked() {
                    audio.normalize = Some("peak".into());
                }
                if ui
                    .selectable_label(percentile.is_some(), "percentile")
                    .clicked()
                    && percentile.is_none()
                {
                    audio.normalize = Some("percentile:98".into());
                }
            })
            .response
            .on_hover_text("Reference level: the loudest frame, or a percentile of frames");
        if let Some(mut p) = percentile
            && ui
                .add(egui::DragValue::new(&mut p).range(0.0..=100.0).speed(0.2))
                .changed()
        {
            audio.normalize = Some(format!("percentile:{p}"));
        }
    });
    let mut sensitivity = audio.onset_sensitivity.unwrap_or(1.5);
    if ui
        .add(egui::Slider::new(&mut sensitivity, 0.5..=4.0).text("Onset sensitivity"))
        .on_hover_text("Higher values need sharper hits to count as onsets")
        .changed()
    {
        audio.onset_sensitivity = Some(sensitivity);
    }

    if audio != original || !renames.is_empty() {
        file.audio = Some(audio);
        true
    } else {
        false
    }
}

fn modulations(ui: &mut egui::Ui, file: &mut MappingFile, catalog: &ParamCatalog) -> bool {
    let bands: Vec<String> = file
        .audio
        .as_ref()
        .map(|a| a.bands.keys().cloned().collect())
        .unwrap_or_default();
    let mut changed = false;
    let mut removed = None;
    for (index, modulation) in file.modulate.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("#{}", index + 1));
                    changed |= target_picker(ui, &mut modulation.target, catalog);
                    if ui
                        .small_button("×")
                        .on_hover_text("Remove modulation")
                        .clicked()
                    {
                        removed = Some(index);
                    }
                });
                if changed {
                    fit_to_target(modulation, catalog);
                }
                changed |= modulation_body(ui, modulation, catalog, &bands);
            });
        });
    }
    if let Some(index) = removed {
        file.modulate.remove(index);
        changed = true;
    }
    if ui.button("Add modulation").clicked() {
        file.modulate.push(ModulationSection {
            target: "snow_intensity".into(),
            source: "rms".into(),
            range: None,
            threshold: None,
            curve: None,
            attack_ms: 0.0,
            release_ms: 150.0,
            invert: false,
        });
        changed = true;
    }
    changed
}

/// Makes range/threshold valid for the (new) target.
fn fit_to_target(m: &mut ModulationSection, catalog: &ParamCatalog) {
    let Some(param) = catalog.get(&m.target) else {
        return;
    };
    if param.kind.is_bool() {
        m.range = None;
        m.threshold.get_or_insert(0.5);
    } else {
        m.threshold = None;
        let (min, max) = param.kind.bounds();
        if let Some(range) = &mut m.range {
            for v in range {
                *v = (*v as f64).clamp(min, max) as f32;
            }
        }
    }
}

fn param_title(param: &Param) -> String {
    format!("{}  ({})", param.label, param.name)
}

/// A parameter's label with its ntsc-rs group, such as "Chroma noise: Intensity". bender-rs
/// labels already name their stage.
pub fn param_label(catalog: &ParamCatalog, index: usize) -> String {
    let param = catalog.param(index);
    catalog
        .sections()
        .iter()
        .find(|s| {
            !s.bender
                && s.toggle.is_some_and(|t| t != index)
                && s.params.iter().any(|&(i, _)| i == index)
        })
        .map_or_else(
            || param.label.to_string(),
            |s| format!("{}: {}", s.label, param.label),
        )
}

fn section_title(section: &ParamSection) -> String {
    if section.bender {
        format!("bender-rs · {}", section.label)
    } else if section.toggle.is_none() {
        "ntsc-rs · general".to_string()
    } else {
        format!("ntsc-rs · {}", section.label)
    }
}

/// Searchable catalog picker. Returns true when the target changed.
fn target_picker(ui: &mut egui::Ui, target: &mut String, catalog: &ParamCatalog) -> bool {
    let selected = catalog
        .index_of(target)
        .map_or_else(|| format!("unknown: {target}"), |i| param_label(catalog, i));
    let search_id = ui.make_persistent_id("target-search");
    let mut changed = false;
    let response = egui::ComboBox::from_id_salt("target")
        .width(250.0)
        .height(360.0)
        .selected_text(selected)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let mut query = ui
                .data(|d| d.get_temp::<String>(search_id))
                .unwrap_or_default();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text("Search parameters…")
                    .desired_width(f32::INFINITY),
            );
            if !edit.has_focus() && query.is_empty() {
                edit.request_focus();
            }
            if edit.changed() {
                ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
            }
            let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
            let matches = |p: &Param| {
                let hay = format!("{} {}", p.label, p.name).to_lowercase();
                terms.iter().all(|t| hay.contains(t))
            };
            for section in catalog.sections() {
                let members: Vec<&Param> = section
                    .toggle
                    .into_iter()
                    .chain(section.params.iter().map(|&(i, _)| i))
                    .map(|i| catalog.param(i))
                    .filter(|p| matches(p))
                    .collect();
                if members.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new(section_title(section)).strong().small());
                for param in members {
                    let mut item = ui.selectable_label(param.name == target, param_title(param));
                    if let Some(desc) = param.description {
                        item = item.on_hover_text(desc);
                    }
                    if item.clicked() {
                        *target = param.name.to_string();
                        changed = true;
                        ui.data_mut(|d| d.remove::<String>(search_id));
                        ui.close();
                    }
                }
            }
        })
        .response;
    if let Some(param) = catalog.get(target) {
        response.on_hover_text(format!("{}\n{}", param.name, param.kind.describe()));
    }
    changed
}

fn modulation_body(
    ui: &mut egui::Ui,
    m: &mut ModulationSection,
    catalog: &ParamCatalog,
    bands: &[String],
) -> bool {
    let before = m.clone();
    egui::Grid::new("modulation")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Source");
            egui::ComboBox::from_id_salt("source")
                .selected_text(m.source.clone())
                .show_ui(ui, |ui| {
                    let sources = ["rms".to_string(), "onset".to_string()]
                        .into_iter()
                        .chain(bands.iter().map(|b| format!("band:{b}")));
                    for source in sources {
                        let selected = m.source == source;
                        if ui.selectable_label(selected, &source).clicked() {
                            m.source = source;
                        }
                    }
                });
            ui.end_row();

            match catalog.get(&m.target) {
                Some(param) if param.kind.is_bool() => {
                    ui.label("Threshold");
                    let mut threshold = m.threshold.unwrap_or(0.5);
                    if ui
                        .add(egui::Slider::new(&mut threshold, 0.0..=1.0))
                        .on_hover_text("The target switches on when the shaped source reaches this")
                        .changed()
                    {
                        m.threshold = Some(threshold);
                    }
                    ui.end_row();
                }
                Some(param) => {
                    ui.label("Range");
                    let (min, max) = param.kind.bounds();
                    let [mut lo, mut hi] = m.range.unwrap_or([min as f32, max as f32]);
                    let speed = (max - min) / 400.0;
                    ui.horizontal(|ui| {
                        let a = ui.add(egui::DragValue::new(&mut lo).range(min..=max).speed(speed));
                        ui.label("→");
                        let b = ui.add(egui::DragValue::new(&mut hi).range(min..=max).speed(speed));
                        if a.changed() || b.changed() {
                            m.range = Some([lo, hi]);
                        }
                        ui.weak(format!("{min}..{max}"))
                            .on_hover_text(param.kind.describe());
                    });
                    ui.end_row();
                }
                None => {}
            }

            ui.label("Curve");
            ui.horizontal(|ui| curve_editor(ui, &mut m.curve));
            ui.end_row();

            ui.label("Smoothing");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut m.attack_ms)
                        .range(0.0..=5000.0)
                        .speed(1.0)
                        .prefix("attack ")
                        .suffix(" ms"),
                );
                ui.add(
                    egui::DragValue::new(&mut m.release_ms)
                        .range(0.0..=5000.0)
                        .speed(2.0)
                        .prefix("release ")
                        .suffix(" ms"),
                );
            });
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut m.invert, "Invert source");
            ui.end_row();
        });
    *m != before
}

fn curve_editor(ui: &mut egui::Ui, curve: &mut Option<String>) {
    let text = curve.clone().unwrap_or_else(|| "linear".into());
    let parsed = Curve::parse(&text).ok();
    let label = match parsed {
        Some(Curve::Linear) => "linear",
        Some(Curve::Pow(_)) => "pow",
        Some(Curve::Exp) => "exp",
        Some(Curve::Log) => "log",
        None => "invalid",
    };
    egui::ComboBox::from_id_salt("curve")
        .selected_text(label)
        .show_ui(ui, |ui| {
            for (name, value) in [
                ("linear", None),
                ("pow", Some("pow:2")),
                ("exp", Some("exp")),
                ("log", Some("log")),
            ] {
                if ui.selectable_label(label == name, name).clicked() && label != name {
                    *curve = value.map(str::to_string);
                }
            }
        });
    if let Some(Curve::Pow(mut k)) = parsed
        && ui
            .add(
                egui::DragValue::new(&mut k)
                    .range(0.05..=16.0)
                    .speed(0.02)
                    .prefix("k "),
            )
            .changed()
    {
        *curve = Some(format!("pow:{k}"));
    }
}

fn base_look(ui: &mut egui::Ui, doc: &mut MappingDoc, catalog: &ParamCatalog) -> bool {
    let mut changed = false;
    let base = doc.file.base.clone().unwrap_or_default();
    let bender = doc.file.bender.clone().unwrap_or_default();
    let mut new_base = base.clone();
    let mut new_bender = bender.clone();
    let ntsc_on = base.ntsc.unwrap_or(true);

    let mut on = ntsc_on;
    if ui
        .checkbox(&mut on, "ntsc-rs enabled")
        .on_hover_text("Off renders bender-rs bends only")
        .changed()
    {
        new_base.ntsc = (!on).then_some(false);
    }
    ui.add_enabled_ui(ntsc_on, |ui| {
        optional_seed(ui, "ntsc-rs seed", &mut new_base.seed);
        if let Some(path) = preset_row(
            ui,
            "ntsc-rs preset",
            &mut new_base.preset,
            "Load ntsc-rs preset…",
        ) {
            new_base.preset = Some(doc.preset_reference(&path));
        }
    });
    ui.horizontal(|ui| {
        ui.label("bender-rs runs");
        let mut order = bender.order.unwrap_or_default();
        egui::ComboBox::from_id_salt("bend-order")
            .selected_text(match order {
                BendOrder::Before => "before ntsc-rs",
                BendOrder::After => "after ntsc-rs",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut order, BendOrder::Before, "before ntsc-rs");
                ui.selectable_value(&mut order, BendOrder::After, "after ntsc-rs");
            });
        if order != bender.order.unwrap_or_default() {
            new_bender.order = Some(order);
        }
    });
    optional_seed(ui, "bender-rs seed", &mut new_bender.seed);
    if let Some(path) = preset_row(
        ui,
        "bender-rs preset",
        &mut new_bender.preset,
        "Load bender-rs preset…",
    ) {
        new_bender.preset = Some(doc.preset_reference(&path));
    }
    if new_base != base {
        doc.file.base = Some(new_base);
        changed = true;
    }
    if new_bender != bender {
        doc.file.bender = Some(new_bender);
        changed = true;
    }

    ui.separator();
    ui.weak("Values not set in the mapping come from the preset or the default.");
    let defaults = EffectSettings::default();
    let values = doc.compiled.as_ref().map_or(&defaults, |c| &c.base);
    let modulated: BTreeSet<&str> = doc
        .file
        .modulate
        .iter()
        .map(|m| m.target.as_str())
        .collect();
    let ntsc_on = doc.file.base.as_ref().and_then(|b| b.ntsc).unwrap_or(true);
    let mut edits = Vec::new();
    for (index, section) in catalog.sections().iter().enumerate() {
        let enabled = section.bender || ntsc_on;
        let id = ui.make_persistent_id(("section", index));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                ui.add_enabled_ui(enabled, |ui| match section.toggle {
                    Some(toggle) => {
                        let param = catalog.param(toggle);
                        param_widget(
                            ui,
                            &doc.file,
                            param,
                            values,
                            &modulated,
                            section.label,
                            &mut edits,
                        );
                    }
                    None => {
                        ui.label(egui::RichText::new(section.label).strong());
                    }
                })
                .response
                .on_hover_text(section.description.unwrap_or(""));
                if section.bender {
                    ui.weak("bender-rs");
                }
            })
            .body(|ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    for &(param_index, depth) in &section.params {
                        let param = catalog.param(param_index);
                        if param.name == "random_seed" {
                            continue;
                        }
                        let label = param
                            .label
                            .strip_prefix(section.label)
                            .and_then(|rest| rest.strip_prefix(": "))
                            .unwrap_or(param.label);
                        ui.horizontal(|ui| {
                            ui.add_space(depth as f32 * 14.0);
                            param_widget(
                                ui, &doc.file, param, values, &modulated, label, &mut edits,
                            );
                        });
                    }
                });
            });
    }
    for (param, edit) in edits {
        match edit {
            Some(value) => doc.file.set_base_value(param, value),
            None => doc.file.clear_base_value(param),
        }
        changed = true;
    }
    changed
}

fn optional_seed(ui: &mut egui::Ui, label: &str, seed: &mut Option<i32>) {
    ui.horizontal(|ui| {
        let mut set = seed.is_some();
        if ui
            .checkbox(&mut set, label)
            .on_hover_text("Fix the random seed; unset uses the preset's")
            .changed()
        {
            *seed = set.then_some(0);
        }
        if let Some(value) = seed {
            ui.add(egui::DragValue::new(value));
        }
    });
}

/// Shows a preset path with load and clear buttons. Returns a newly chosen file.
fn preset_row(
    ui: &mut egui::Ui,
    label: &str,
    preset: &mut Option<PathBuf>,
    button: &str,
) -> Option<PathBuf> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(label);
        match preset.as_deref() {
            Some(path) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into(),
                );
                ui.monospace(name).on_hover_text(path.display().to_string());
                if ui
                    .small_button("×")
                    .on_hover_text("Remove preset")
                    .clicked()
                {
                    *preset = None;
                }
            }
            None => {
                ui.weak("none");
            }
        }
    });
    if ui.button(button).clicked() {
        chosen = rfd::FileDialog::new()
            .add_filter("JSON preset", &["json"])
            .pick_file();
    }
    chosen
}

/// Whether the mapping sets `param` itself (rather than inheriting it).
fn is_set(file: &MappingFile, param: &Param) -> bool {
    match param.name.strip_prefix(BENDER_PREFIX) {
        Some(id) => file.bender.as_ref().is_some_and(|b| b.set.contains_key(id)),
        None => file
            .base
            .as_ref()
            .is_some_and(|b| b.set.contains_key(param.name)),
    }
}

/// A float written to TOML at f32 precision, so `0.1` stays `0.1`.
fn tidy_float(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(value as f64)
}

/// One base-value widget, after bender's `setting_widget`. Queues `Some(value)` to set or
/// `None` to clear the mapping's value.
fn param_widget<'c>(
    ui: &mut egui::Ui,
    file: &MappingFile,
    param: &'c Param,
    values: &EffectSettings,
    modulated: &BTreeSet<&str>,
    label: &str,
    edits: &mut Vec<(&'c Param, Option<f64>)>,
) {
    let current = get_param(values, param);
    let is_modulated = modulated.contains(param.name);
    let mut text = egui::RichText::new(label);
    if is_modulated {
        text = text.color(ui.visuals().warn_fg_color);
    }
    let (response, edited) = match &param.kind {
        ParamKind::Float {
            min,
            max,
            logarithmic,
        } => {
            let mut v = current as f32;
            let r = ui.add(
                egui::Slider::new(&mut v, *min..=*max)
                    .logarithmic(*logarithmic)
                    .text(text),
            );
            let changed = r.changed();
            (r, changed.then(|| tidy_float(v)))
        }
        ParamKind::Int { min, max } => {
            let mut v = current.round() as i32;
            let r = ui.add(egui::Slider::new(&mut v, *min..=*max).text(text));
            let changed = r.changed();
            (r, changed.then_some(f64::from(v)))
        }
        ParamKind::Bool => {
            let mut v = current >= 0.5;
            let r = ui.checkbox(&mut v, text);
            let changed = r.changed();
            (r, changed.then_some(f64::from(v)))
        }
        ParamKind::Enum { options } => {
            let before = current.round() as usize;
            let mut v = before;
            let selected = options.get(v).map_or("?", |o| o.1.as_str());
            let r = egui::ComboBox::from_id_salt(param.name)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (i, (_, option)) in options.iter().enumerate() {
                        ui.selectable_value(&mut v, i, option);
                    }
                })
                .response;
            ui.label(text);
            (r, (v != before).then_some(v as f64))
        }
    };
    let mut hover = param.description.unwrap_or("").to_string();
    if is_modulated {
        hover.push_str("\n\nModulated: this is the base value the modulations start from.");
    }
    hover.push_str(&format!("\n\n{}", param.name));
    response.on_hover_text(hover.trim_start());
    if let Some(value) = edited {
        edits.push((param, Some(value)));
    }
    if is_set(file, param)
        && ui
            .small_button("reset")
            .on_hover_text("Remove from the mapping (use the preset or default)")
            .clicked()
    {
        edits.push((param, None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_climb_to_the_common_ancestor() {
        assert_eq!(
            relative_to(Path::new("/a/b/presets/p.json"), Path::new("/a/b")),
            Some(PathBuf::from("presets/p.json"))
        );
        assert_eq!(
            relative_to(Path::new("/a/assets/p.json"), Path::new("/a/maps/x")),
            Some(PathBuf::from("../../assets/p.json"))
        );
        assert_eq!(relative_to(Path::new("/x/p.json"), Path::new("/y")), None);
        assert_eq!(relative_to(Path::new("p.json"), Path::new("/y")), None);
    }

    #[test]
    fn saving_rebases_presets_onto_the_new_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute(dir.path());
        std::fs::create_dir_all(root.join("maps/deep")).unwrap();
        std::fs::write(root.join("p.json"), "{}").unwrap();
        std::fs::write(root.join("maps/m.toml"), "[base]\npreset = \"../p.json\"\n").unwrap();
        let mut doc = MappingDoc::open(&root.join("maps/m.toml")).unwrap();
        doc.save_as(&root.join("maps/deep/n.toml")).unwrap();
        let preset = doc.file.base.as_ref().unwrap().preset.clone().unwrap();
        assert_eq!(preset, PathBuf::from("../../p.json"));
        assert!(!doc.is_dirty());
        let text = std::fs::read_to_string(root.join("maps/deep/n.toml")).unwrap();
        assert!(text.contains("../../p.json"), "{text}");
    }
}

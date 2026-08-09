//! System font discovery on Android.
//!
//! Android has no fontconfig, so `font_config.rs` cannot be reused. What it
//! does have is a well-known set of directories holding the system fonts and a
//! configuration file, `/system/etc/fonts.xml`, that names the families and,
//! crucially, gives the *order* in which families should be consulted when a
//! codepoint is not covered by the requested font. That ordering is what makes
//! CJK and emoji work; without it a terminal on Android sees only the vendored
//! fonts, none of which have CJK coverage.
//!
//! The whole cell-grid alignment problem that `wezterm-font` solves — wide
//! characters, Nerd Font fallback, grid alignment — does not go away here. All
//! that changes is where the fonts come from.
//!
//! Parsing is deliberately tolerant. `fonts.xml` has had three formats over
//! Android's life, vendors modify it, and a device may have none of it; every
//! step degrades to "just enumerate the directories" rather than failing.

use crate::locator::{FontDataSource, FontLocator, FontOrigin};
use crate::parser::{best_matching_font, parse_and_collect_font_info, ParsedFont};
use config::FontAttributes;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where Android keeps fonts. `/system/fonts` is universal; the others appear
/// on Treble devices and on some vendor images.
const FONT_DIRS: &[&str] = &[
    "/system/fonts",
    "/system/font",
    "/product/fonts",
    "/system_ext/fonts",
    "/vendor/fonts",
    "/data/fonts",
];

/// Candidate locations for the family/fallback configuration, newest layout
/// first.
const FONTS_XML: &[&str] = &[
    "/system/etc/fonts.xml",
    "/vendor/etc/fonts.xml",
    "/etc/fonts.xml",
    // Pre-Lollipop split the config in two. Only the fallback half matters to
    // us, and its <fileset> shape is handled by the same parser.
    "/system/etc/system_fonts.xml",
    "/system/etc/fallback_fonts.xml",
];

#[derive(Debug, Clone)]
struct FontEntry {
    path: PathBuf,
    /// The family name from fonts.xml, if the file was named by one. Used only
    /// to order fallbacks; matching a requested font by name goes through
    /// freetype's own name tables, which are authoritative.
    family: Option<String>,
    /// Position in the fallback chain. Fonts named by `fonts.xml` inherit its
    /// ordering; anything found only by scanning a directory sorts last.
    rank: usize,
}

pub struct AndroidFontLocator {
    /// The parsed system font list. Built once; scanning the font directories
    /// and parsing a hundred-odd files is not something to repeat per query.
    entries: Mutex<Option<Vec<FontEntry>>>,
}

impl AndroidFontLocator {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(None),
        }
    }

    fn entries(&self) -> Vec<FontEntry> {
        let mut cache = self.entries.lock().unwrap();
        if cache.is_none() {
            let entries = discover_system_fonts();
            log::debug!("discovered {} system fonts", entries.len());
            cache.replace(entries);
        }
        cache.as_ref().expect("just populated").clone()
    }
}

impl Default for AndroidFontLocator {
    fn default() -> Self {
        Self::new()
    }
}

impl FontLocator for AndroidFontLocator {
    fn load_fonts(
        &self,
        fonts_selection: &[FontAttributes],
        loaded: &mut HashSet<FontAttributes>,
        pixel_size: u16,
    ) -> anyhow::Result<Vec<ParsedFont>> {
        let entries = self.entries();
        let mut result = vec![];

        for attr in fonts_selection {
            if loaded.contains(attr) {
                continue;
            }

            let mut best: Option<ParsedFont> = None;

            // Try files that fonts.xml attributed to a matching family first.
            // Matching by name is still done properly, by freetype against the
            // font's own name tables, but this usually finds the answer in one
            // parse instead of a hundred.
            let ordered = order_by_family_hint(&entries, &attr.family);

            for entry in ordered {
                let source = FontDataSource::OnDisk(entry.path.clone());
                let origin = FontOrigin::FontDirs;

                match best_matching_font(&source, attr, origin, pixel_size) {
                    Ok(Some(parsed)) => {
                        // Prefer the earlier entry when two files match
                        // equally well: fonts.xml order encodes the vendor's
                        // own preference.
                        if best.is_none() {
                            best.replace(parsed);
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        log::trace!("failed to parse {}: {err:#}", entry.path.display());
                    }
                }

                if best.is_some() {
                    break;
                }
            }

            if let Some(parsed) = best {
                log::trace!("resolved {attr:?} to {}", parsed.handle.diagnostic_string());
                loaded.insert(attr.clone());
                result.push(parsed);
            }
        }

        Ok(result)
    }

    fn enumerate_all_fonts(&self) -> anyhow::Result<Vec<ParsedFont>> {
        let mut fonts = vec![];

        for entry in self.entries() {
            let source = FontDataSource::OnDisk(entry.path.clone());
            if let Err(err) = parse_and_collect_font_info(&source, &mut fonts, FontOrigin::FontDirs)
            {
                log::trace!("failed to parse {}: {err:#}", entry.path.display());
            }
        }

        fonts.sort();
        fonts.dedup();
        Ok(fonts)
    }

    fn locate_fallback_for_codepoints(
        &self,
        codepoints: &[char],
    ) -> anyhow::Result<Vec<ParsedFont>> {
        let entries = self.entries();
        let mut fonts: Vec<ParsedFont> = vec![];

        // Walk the fallback chain in the order fonts.xml gave us and take the
        // first font that covers each codepoint. Codepoints are handled one at
        // a time because no single font is likely to cover a mixed set, but a
        // font already chosen for an earlier codepoint is checked first so
        // that printing a whole unicode block does not rescan the chain for
        // every character.
        'next_codepoint: for &c in codepoints {
            let mut wanted = rangeset::RangeSet::new();
            wanted.add(c as u32);

            for f in &fonts {
                if matches!(f.coverage_intersection(&wanted), Ok(r) if !r.is_empty()) {
                    continue 'next_codepoint;
                }
            }

            for entry in &entries {
                let source = FontDataSource::OnDisk(entry.path.clone());
                let mut candidates = vec![];
                if parse_and_collect_font_info(&source, &mut candidates, FontOrigin::FontDirs)
                    .is_err()
                {
                    continue;
                }

                for candidate in candidates {
                    match candidate.coverage_intersection(&wanted) {
                        Ok(r) if !r.is_empty() => {
                            log::trace!(
                                "fallback for U+{:04X} is {}",
                                c as u32,
                                candidate.handle.diagnostic_string()
                            );
                            fonts.push(candidate);
                            continue 'next_codepoint;
                        }
                        _ => {}
                    }
                }
            }

            log::trace!("no system font covers U+{:04X}", c as u32);
        }

        Ok(fonts)
    }
}

/// Reorder `entries` so that any file `fonts.xml` attributed to a family
/// matching `family` is considered first, preserving the original order
/// within each group.
fn order_by_family_hint<'a>(entries: &'a [FontEntry], family: &str) -> Vec<&'a FontEntry> {
    let (hinted, rest): (Vec<_>, Vec<_>) = entries.iter().partition(|entry| {
        entry
            .family
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(family))
    });

    hinted.into_iter().chain(rest).collect()
}

/// Build the ordered system font list: everything named by `fonts.xml`, in the
/// order it names them, followed by anything else found in the font
/// directories.
fn discover_system_fonts() -> Vec<FontEntry> {
    let mut entries: Vec<FontEntry> = vec![];
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for (rank, (family, filename)) in parse_fonts_xml().into_iter().enumerate() {
        if let Some(path) = resolve_font_file(&filename) {
            if seen.insert(path.clone()) {
                entries.push(FontEntry {
                    path,
                    family,
                    rank,
                });
            }
        }
    }

    // Whatever fonts.xml did not mention -- vendor additions, user-dropped
    // files -- still belongs in the list, just at lower priority.
    let base_rank = entries.len();
    for dir in FONT_DIRS {
        let dir = Path::new(dir);
        let read_dir = match std::fs::read_dir(dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };

        let mut extra: Vec<PathBuf> = read_dir
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| is_font_file(path))
            .filter(|path| !seen.contains(path))
            .collect();
        // Directory order is not stable, so sort for a deterministic
        // fallback chain across runs.
        extra.sort();

        for path in extra {
            seen.insert(path.clone());
            entries.push(FontEntry {
                path,
                family: None,
                rank: base_rank,
            });
        }
    }

    entries.sort_by_key(|entry| entry.rank);
    entries
}

fn is_font_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc" | "otc"
        ),
        None => false,
    }
}

fn resolve_font_file(filename: &str) -> Option<PathBuf> {
    // fonts.xml names bare filenames; a few vendor files give absolute paths.
    if filename.starts_with('/') {
        let path = PathBuf::from(filename);
        return path.is_file().then_some(path);
    }

    for dir in FONT_DIRS {
        let path = Path::new(dir).join(filename);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Extract `(family, filename)` pairs from the first readable `fonts.xml`, in
/// document order.
///
/// The modern format is:
///
/// ```xml
/// <familyset>
///   <family name="sans-serif">
///     <font weight="400" style="normal">Roboto-Regular.ttf</font>
///   </family>
///   <family lang="ja">             <!-- unnamed families are fallbacks -->
///     <font>NotoSansCJK-Regular.ttc</font>
///   </family>
/// </familyset>
/// ```
///
/// The pre-Lollipop format nests `<fileset><file>` inside `<family>` instead.
/// Both are handled by treating any character data inside a `font` or `file`
/// element as a filename, which also means an unrecognised vendor variation
/// degrades to "found fewer fonts" rather than to an error.
fn parse_fonts_xml() -> Vec<(Option<String>, String)> {
    for candidate in FONTS_XML {
        let text = match std::fs::read_to_string(candidate) {
            Ok(text) => text,
            Err(_) => continue,
        };

        match parse_fonts_xml_text(&text) {
            Ok(fonts) if !fonts.is_empty() => {
                log::debug!("{} named {} fonts", candidate, fonts.len());
                return fonts;
            }
            Ok(_) => {}
            Err(err) => {
                log::warn!("failed to parse {candidate}: {err:#}");
            }
        }
    }

    log::debug!("no usable fonts.xml; falling back to directory enumeration");
    vec![]
}

fn parse_fonts_xml_text(text: &str) -> anyhow::Result<Vec<(Option<String>, String)>> {
    use xml::reader::{EventReader, XmlEvent};

    let mut fonts = vec![];
    let mut current_family: Option<String> = None;
    // The name of the element we are collecting character data for, if any.
    let mut in_font_element = false;
    let mut pending = String::new();

    for event in EventReader::from_str(text) {
        match event? {
            XmlEvent::StartElement {
                name, attributes, ..
            } => match name.local_name.as_str() {
                "family" => {
                    current_family = attributes
                        .iter()
                        .find(|attr| attr.name.local_name == "name")
                        .map(|attr| attr.value.clone());
                }
                "font" | "file" => {
                    in_font_element = true;
                    pending.clear();
                }
                _ => {}
            },

            XmlEvent::Characters(text) | XmlEvent::CData(text) if in_font_element => {
                pending.push_str(&text);
            }

            XmlEvent::EndElement { name } => match name.local_name.as_str() {
                "family" => {
                    current_family = None;
                }
                "font" | "file" => {
                    in_font_element = false;
                    let filename = pending.trim();
                    if !filename.is_empty() {
                        fonts.push((current_family.clone(), filename.to_string()));
                    }
                    pending.clear();
                }
                _ => {}
            },

            _ => {}
        }
    }

    Ok(fonts)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_the_modern_format() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<familyset version="23">
  <family name="sans-serif">
    <font weight="400" style="normal">Roboto-Regular.ttf</font>
    <font weight="700" style="normal">Roboto-Bold.ttf</font>
  </family>
  <family lang="ja">
    <font weight="400" style="normal" index="0">NotoSansCJK-Regular.ttc</font>
  </family>
</familyset>"#;

        let fonts = parse_fonts_xml_text(xml).unwrap();
        assert_eq!(
            fonts,
            vec![
                (Some("sans-serif".to_string()), "Roboto-Regular.ttf".to_string()),
                (Some("sans-serif".to_string()), "Roboto-Bold.ttf".to_string()),
                (None, "NotoSansCJK-Regular.ttc".to_string()),
            ]
        );
    }

    #[test]
    fn parses_the_pre_lollipop_format() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<familyset>
  <family>
    <nameset><name>sans-serif</name></nameset>
    <fileset>
      <file>Roboto-Regular.ttf</file>
      <file>Roboto-Bold.ttf</file>
    </fileset>
  </family>
</familyset>"#;

        let fonts = parse_fonts_xml_text(xml).unwrap();
        assert_eq!(
            fonts,
            vec![
                (None, "Roboto-Regular.ttf".to_string()),
                (None, "Roboto-Bold.ttf".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_malformed_entries_without_failing() {
        let xml = r#"<familyset>
  <family name="sans-serif">
    <font></font>
    <font>   </font>
    <font>Roboto-Regular.ttf</font>
  </family>
</familyset>"#;

        let fonts = parse_fonts_xml_text(xml).unwrap();
        assert_eq!(
            fonts,
            vec![(
                Some("sans-serif".to_string()),
                "Roboto-Regular.ttf".to_string()
            )]
        );
    }

    #[test]
    fn recognises_font_file_extensions() {
        assert!(is_font_file(Path::new("/system/fonts/Roboto-Regular.ttf")));
        assert!(is_font_file(Path::new("/system/fonts/NotoSansCJK.ttc")));
        assert!(is_font_file(Path::new("/system/fonts/Thing.OTF")));
        assert!(!is_font_file(Path::new("/system/fonts/fonts.xml")));
        assert!(!is_font_file(Path::new("/system/fonts/README")));
    }
}

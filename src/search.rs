//! Search menu bar items (v1.11), the pure half: which items match a query,
//! the rows of the result list and their geometry. `platform::search_panel`
//! shows the list.
//!
//! As in Ice's search panel, an empty query lists every item under its
//! section, right to left within each section, the way the sections spread
//! out from the icon; a query lists only the matching items, best first.
//! Ice matches with a fuzzy search that forgives typos; here each word of
//! the query must match a name's start, a word in it, a part of it, or
//! letters of it in order.

use crate::arrange::{BarModel, Tile};
use crate::model::{Rect, WindowId};
use crate::sections::Section;

/// Height of a section header row, in points.
pub const HEADER_HEIGHT: f64 = 28.0;
/// Height of an item row, in points.
pub const ITEM_HEIGHT: f64 = 36.0;
/// Space above the first row and below the last.
pub const LIST_PADDING: f64 = 8.0;
/// Size of an app icon in an item row.
pub const ICON_SIZE: f64 = 24.0;
/// Item images taller than this are scaled down to it.
pub const IMAGE_HEIGHT: f64 = 22.0;
/// Height of one line of row text.
const TEXT_HEIGHT: f64 = 17.0;
/// Control Center's indicators for the camera, microphone or screen in
/// use, FaceTime and music recognition. They come and go and cannot be
/// hidden, and Ice leaves them out of its search too.
const UNLISTED: [&str; 3] = ["AudioVideoModule", "FaceTime", "MusicRecognition"];

/// An item the search can find.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: WindowId,
    pub section: Section,
    pub name: String,
}

/// One row of the result list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Header(Section),
    /// An index into the entries.
    Item(usize),
}

impl Row {
    pub fn height(self) -> f64 {
        match self {
            Row::Header(_) => HEADER_HEIGHT,
            Row::Item(_) => ITEM_HEIGHT,
        }
    }
}

/// The items of `model` in list order: by section, right to left within
/// each, without `skip` (ice-rust's icon) and `UNLISTED`. `name` gives
/// each tile's name.
pub fn entries(
    model: &BarModel,
    skip: Option<WindowId>,
    name: impl Fn(&Tile) -> String,
) -> Vec<Entry> {
    Section::ALL
        .iter()
        .flat_map(|&section| {
            model
                .bar(section)
                .iter()
                .rev()
                .filter(|tile| Some(tile.id) != skip && !UNLISTED.contains(&tile.title.as_str()))
                .map(|tile| Entry {
                    id: tile.id,
                    section,
                    name: name(tile),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The rows for `query`: every entry under its section's header when the
/// query is blank, otherwise the matching entries alone, best first and in
/// list order among equals. `entries` are in list order.
pub fn rows(entries: &[Entry], query: &str) -> Vec<Row> {
    if query.trim().is_empty() {
        let mut rows = Vec::with_capacity(entries.len() + 3);
        for section in Section::ALL {
            let mut items = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.section == section)
                .map(|(i, _)| Row::Item(i))
                .peekable();
            if items.peek().is_some() {
                rows.push(Row::Header(section));
                rows.extend(items);
            }
        }
        return rows;
    }
    let mut found: Vec<(u32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| Some((score(&e.name, query)?, i)))
        .collect();
    // Stable: equal scores keep list order.
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, i)| Row::Item(i)).collect()
}

/// How well `name` matches `query`, higher is better, or `None` when it
/// does not match. Case does not count, and every word of the query must
/// match (`word_score`). A blank query matches everything equally.
pub fn score(name: &str, query: &str) -> Option<u32> {
    query
        .split_whitespace()
        .try_fold(0, |total, word| Some(total + word_score(name, word)?))
}

const AT_START: u32 = 400;
const AT_WORD: u32 = 300;
const INSIDE: u32 = 200;
const IN_ORDER: u32 = 100;
/// Added for each letter of an in-order match that starts a word.
const WORD_LETTER: u32 = 10;

/// One query word against `name`: at its start, at the start of a word in
/// it ("Machine" in "Time Machine", "Machine" in "TimeMachine"), anywhere
/// in it, or as letters in order, more for letters that start words ("tm"
/// in "Time Machine").
fn word_score(name: &str, word: &str) -> Option<u32> {
    let name: Vec<char> = name.chars().collect();
    let lower: Vec<char> = name.iter().map(|&c| fold(c)).collect();
    let word: Vec<char> = word.chars().map(fold).collect();
    if word.is_empty() {
        return Some(0);
    }
    let starts = word_starts(&name);
    let found_at = |i: usize| lower[i..].starts_with(&word);
    if found_at(0) {
        return Some(AT_START);
    }
    if starts.iter().any(|&i| found_at(i)) {
        return Some(AT_WORD);
    }
    if (0..lower.len()).any(found_at) {
        return Some(INSIDE);
    }
    let mut at = 0;
    let mut bonus = 0;
    for &c in &word {
        let i = (at..lower.len()).find(|&i| lower[i] == c)?;
        if starts.contains(&i) {
            bonus += WORD_LETTER;
        }
        at = i + 1;
    }
    Some(IN_ORDER + bonus.min(INSIDE - IN_ORDER - 1))
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Where words start in `name`: its first letter or digit, one after a
/// character that is neither, and a capital after a lowercase letter.
fn word_starts(name: &[char]) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, &c) in name.iter().enumerate() {
        if !c.is_alphanumeric() {
            continue;
        }
        let start = match i.checked_sub(1).map(|p| name[p]) {
            None => true,
            Some(prev) => !prev.is_alphanumeric() || (prev.is_lowercase() && c.is_uppercase()),
        };
        if start {
            starts.push(i);
        }
    }
    starts
}

/// The first item row, the selection when the list changes.
pub fn first_item(rows: &[Row]) -> Option<usize> {
    rows.iter().position(|r| matches!(r, Row::Item(_)))
}

/// The item row after `current` (`down`) or before it, skipping headers;
/// `current` itself at either end, and the first item without a current
/// row.
pub fn step(rows: &[Row], current: Option<usize>, down: bool) -> Option<usize> {
    let Some(current) = current.filter(|&c| c < rows.len()) else {
        return first_item(rows);
    };
    let is_item = |i: &usize| matches!(rows[*i], Row::Item(_));
    let next = if down {
        (current + 1..rows.len()).find(is_item)
    } else {
        (0..current).rev().find(is_item)
    };
    Some(next.unwrap_or(current))
}

/// The row that shows entry `entry`, to keep a selection across a refresh.
pub fn row_of(rows: &[Row], entry: usize) -> Option<usize> {
    rows.iter().position(|&r| r == Row::Item(entry))
}

/// The top of each row from the top of the list, and the list's height.
pub fn row_tops(rows: &[Row]) -> (Vec<f64>, f64) {
    let mut tops = Vec::with_capacity(rows.len());
    let mut y = LIST_PADDING;
    for row in rows {
        tops.push(y);
        y += row.height();
    }
    (tops, y + LIST_PADDING)
}

/// Where the parts of an item row go, in list coordinates (from the top
/// left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemFrames {
    /// The selection highlight.
    pub row: Rect,
    pub icon: Rect,
    pub name: Rect,
    /// The item's image at the right end, when it has one.
    pub image: Option<Rect>,
    /// The fill behind the image.
    pub image_back: Option<Rect>,
}

/// The parts of an item row `top` from the top of a list `width` wide.
/// `image` is the item image's size; images taller than `IMAGE_HEIGHT`
/// are scaled down, and the name ends before the image.
pub fn item_frames(width: f64, top: f64, image: Option<(f64, f64)>) -> ItemFrames {
    let middle = |height: f64| top + (ITEM_HEIGHT - height) / 2.0;
    let image = image.filter(|&(w, h)| w > 0.0 && h > 0.0).map(|(w, h)| {
        let scale = (IMAGE_HEIGHT / h).min(1.0);
        let (w, h) = (w * scale, h * scale);
        Rect::new(width - 20.0 - w, middle(h), w, h)
    });
    let image_back =
        image.map(|r| Rect::new(r.x - 4.0, top + 5.0, r.width + 8.0, ITEM_HEIGHT - 10.0));
    let name_end = image_back.map_or(width - 16.0, |back| back.x - 12.0);
    ItemFrames {
        row: Rect::new(8.0, top + 1.0, (width - 16.0).max(0.0), ITEM_HEIGHT - 2.0),
        icon: Rect::new(16.0, middle(ICON_SIZE), ICON_SIZE, ICON_SIZE),
        name: Rect::new(
            48.0,
            middle(TEXT_HEIGHT),
            (name_end - 48.0).max(0.0),
            TEXT_HEIGHT,
        ),
        image,
        image_back,
    }
}

/// Where a header's text goes, `top` from the top of a list `width` wide.
pub fn header_frame(width: f64, top: f64) -> Rect {
    Rect::new(16.0, top + 10.0, (width - 32.0).max(0.0), 14.0)
}

/// The row at `y` from the top of the list.
pub fn row_at(rows: &[Row], y: f64) -> Option<usize> {
    let (tops, _) = row_tops(rows);
    rows.iter()
        .zip(tops)
        .position(|(row, top)| (top..top + row.height()).contains(&y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrange::Anchors;
    use crate::model::Rect;

    fn entry(id: WindowId, section: Section, name: &str) -> Entry {
        Entry {
            id,
            section,
            name: name.into(),
        }
    }

    /// macOS 26.7.1, Menu Bar Layout on the development host, in part.
    fn sample() -> Vec<Entry> {
        vec![
            entry(1, Section::Visible, "Control Center"),
            entry(2, Section::Visible, "Clock"),
            entry(3, Section::Visible, "WiFi"),
            entry(4, Section::Hidden, "Discord"),
            entry(5, Section::Hidden, "Time Machine"),
            entry(6, Section::Hidden, "Tailscale"),
            entry(7, Section::AlwaysHidden, "Cloudflare WARP"),
        ]
    }

    fn tile(id: WindowId, x: f64, title: &str) -> Tile {
        Tile {
            id,
            title: title.into(),
            bounds: Rect::new(x, 0.0, 30.0, 30.0),
            locked: false,
        }
    }

    #[test]
    fn entries_run_right_to_left_by_section_without_the_icon() {
        let model = BarModel {
            visible: vec![
                tile(9, 852.0, "AudioVideoModule"),
                tile(10, 900.0, "WiFi"),
                tile(11, 930.0, "Icon"),
            ],
            hidden: vec![tile(20, -600.0, "Item-0"), tile(21, -570.0, "Shottr")],
            always_hidden: vec![tile(30, -9000.0, "Bitwarden")],
            anchors: Anchors {
                icon: Some(11),
                hidden_divider: 99,
                always_hidden_divider: Some(98),
            },
        };
        let list = entries(&model, model.anchors.icon, |t| t.title.to_uppercase());
        assert_eq!(
            list,
            vec![
                entry(10, Section::Visible, "WIFI"),
                entry(21, Section::Hidden, "SHOTTR"),
                entry(20, Section::Hidden, "ITEM-0"),
                entry(30, Section::AlwaysHidden, "BITWARDEN"),
            ]
        );
    }

    #[test]
    fn blank_query_lists_every_item_under_its_section() {
        let all = sample();
        let expected = vec![
            Row::Header(Section::Visible),
            Row::Item(0),
            Row::Item(1),
            Row::Item(2),
            Row::Header(Section::Hidden),
            Row::Item(3),
            Row::Item(4),
            Row::Item(5),
            Row::Header(Section::AlwaysHidden),
            Row::Item(6),
        ];
        assert_eq!(rows(&all, ""), expected);
        assert_eq!(rows(&all, "   "), expected);
        // A section without items has no header.
        assert_eq!(
            rows(&all[..3], ""),
            vec![
                Row::Header(Section::Visible),
                Row::Item(0),
                Row::Item(1),
                Row::Item(2)
            ]
        );
        assert!(rows(&[], "").is_empty());
    }

    #[test]
    fn a_query_lists_matches_best_first_without_headers() {
        let all = sample();
        // "c": the start of Control Center, Clock and Cloudflare WARP (in
        // list order), then the names with it inside: Discord, Time
        // Machine, Tailscale.
        assert_eq!(
            rows(&all, "c"),
            vec![
                Row::Item(0),
                Row::Item(1),
                Row::Item(6),
                Row::Item(3),
                Row::Item(4),
                Row::Item(5)
            ]
        );
        assert_eq!(rows(&all, "machine"), vec![Row::Item(4)]);
        assert_eq!(rows(&all, "MACH"), vec![Row::Item(4)]);
        assert_eq!(rows(&all, "zz"), vec![]);
    }

    #[test]
    fn match_kinds_rank_in_order() {
        let start = score("Tailscale", "tail").unwrap();
        let word = score("Time Machine", "mach").unwrap();
        let inside = score("Discord", "cor").unwrap();
        let letters = score("Time Machine", "tmc").unwrap();
        assert!(start > word && word > inside && inside > letters);
        assert_eq!(score("Tailscale", "xyz"), None);
        // Letters out of order do not match.
        assert_eq!(score("Clock", "kc"), None);
    }

    #[test]
    fn word_starts_include_camel_case_and_separators() {
        assert_eq!(score("TimeMachine", "mach"), Some(AT_WORD));
        assert_eq!(score("Cloudflare WARP", "warp"), Some(AT_WORD));
        assert_eq!(score("com.apple.menuextra", "menu"), Some(AT_WORD));
        assert_eq!(score("WiFi", "fi"), Some(AT_WORD));
        // A capital after a capital starts no word: "ARP" is inside "WARP".
        assert_eq!(score("WARP", "arp"), Some(INSIDE));
    }

    #[test]
    fn letters_that_start_words_count_more() {
        let initials = score("Time Machine", "tm").unwrap();
        let scattered = score("Time Machine", "ie").unwrap();
        assert!(initials > scattered, "{initials} {scattered}");
        assert!(initials < INSIDE);
        // Many initials still rank below a match inside the name.
        let long = "A B C D E F G H I J K L M N O P Q R S T U V W";
        assert!(score(long, "abcdefghijklmnopqrstuvw").unwrap() < INSIDE);
    }

    #[test]
    fn every_query_word_must_match() {
        assert!(score("Control Center", "con cen").is_some());
        assert!(score("Control Center", "cen con").is_some());
        assert_eq!(score("Control Center", "con clock"), None);
        let both = score("Control Center", "control center").unwrap();
        assert_eq!(both, AT_START + AT_WORD);
        assert_eq!(score("Clock", ""), Some(0));
    }

    #[test]
    fn case_and_non_ascii_names() {
        assert_eq!(score("ÉCRAN", "écr"), Some(AT_START));
        assert_eq!(score("LINE", "line"), Some(AT_START));
        assert!(score("โปรแกรม", "แกรม").is_some());
    }

    #[test]
    fn steps_skip_headers_and_stop_at_the_ends() {
        let all = sample();
        let list = rows(&all, "");
        assert_eq!(first_item(&list), Some(1));
        assert_eq!(step(&list, Some(1), true), Some(2));
        // Over the Hidden header.
        assert_eq!(step(&list, Some(3), true), Some(5));
        assert_eq!(step(&list, Some(5), false), Some(3));
        // At either end the selection stays.
        assert_eq!(step(&list, Some(9), true), Some(9));
        assert_eq!(step(&list, Some(1), false), Some(1));
        // Without a selection, or a stale one, the first item.
        assert_eq!(step(&list, None, true), Some(1));
        assert_eq!(step(&list, Some(40), false), Some(1));
        assert_eq!(step(&[], None, true), None);
        assert_eq!(first_item(&[Row::Header(Section::Visible)]), None);
    }

    #[test]
    fn selection_follows_its_entry() {
        let all = sample();
        let list = rows(&all, "");
        assert_eq!(row_of(&list, 4), Some(6));
        assert_eq!(row_of(&rows(&all, "time"), 4), Some(0));
        assert_eq!(row_of(&rows(&all, "clock"), 4), None);
    }

    #[test]
    fn item_rows_put_the_icon_left_and_the_image_right() {
        // A 30 pt tall capture from a 30 pt menu bar, 38 pt wide.
        let f = item_frames(584.0, 44.0, Some((38.0, 30.0)));
        assert_eq!(f.row, Rect::new(8.0, 45.0, 568.0, 34.0));
        assert_eq!(f.icon, Rect::new(16.0, 50.0, 24.0, 24.0));
        // Scaled to 22 pt tall, keeping its shape, 20 pt from the right.
        let image = f.image.unwrap();
        assert!((image.width - 38.0 * 22.0 / 30.0).abs() < 1e-9);
        assert_eq!(image.height, 22.0);
        assert!((image.max_x() - 564.0).abs() < 1e-9);
        assert_eq!(image.y, 51.0);
        let back = f.image_back.unwrap();
        assert_eq!((back.y, back.height), (49.0, 26.0));
        assert!((back.width - image.width - 8.0).abs() < 1e-9);
        // The name runs from after the icon to 12 pt before the image.
        assert_eq!(f.name.x, 48.0);
        assert!((f.name.max_x() - (back.x - 12.0)).abs() < 1e-9);
        assert_eq!(f.name.y, 53.5);
    }

    #[test]
    fn small_images_keep_their_size_and_rows_without_one_widen_the_name() {
        let f = item_frames(584.0, 0.0, Some((20.0, 16.0)));
        assert_eq!(f.image, Some(Rect::new(544.0, 10.0, 20.0, 16.0)));
        let f = item_frames(584.0, 0.0, None);
        assert_eq!((f.image, f.image_back), (None, None));
        assert_eq!(f.name.max_x(), 568.0);
        assert_eq!(item_frames(584.0, 0.0, Some((0.0, 30.0))).image, None);
        assert_eq!(header_frame(584.0, 8.0), Rect::new(16.0, 18.0, 552.0, 14.0));
    }

    #[test]
    fn rows_stack_from_the_top_with_padding() {
        let list = vec![Row::Header(Section::Hidden), Row::Item(0), Row::Item(1)];
        let (tops, height) = row_tops(&list);
        assert_eq!(tops, vec![8.0, 36.0, 72.0]);
        assert_eq!(height, 8.0 + 28.0 + 36.0 + 36.0 + 8.0);
        assert_eq!(row_at(&list, 4.0), None);
        assert_eq!(row_at(&list, 8.0), Some(0));
        assert_eq!(row_at(&list, 35.9), Some(0));
        assert_eq!(row_at(&list, 36.0), Some(1));
        assert_eq!(row_at(&list, 107.9), Some(2));
        assert_eq!(row_at(&list, 108.0), None);
        assert_eq!(row_tops(&[]), (vec![], 16.0));
    }
}

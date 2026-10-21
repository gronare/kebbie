use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui::layout::Size;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, FontSize, Image, Resize};

use crate::keys::{self, Act};
use crate::lead;
use super::App;

pub(super) enum Graphics {
    Ready(Picker),
    Pending { wanted: bool },
    Off,
}

pub(super) struct ImageAt {
    pub(super) line: usize,
    pub(super) path: PathBuf,
    pub(super) rows: u16,
    pub(super) area: Size,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Shown {
    Sharp,
    Preview,
    Path,
}

pub(super) fn image_shown(protocol: Option<ProtocolType>, decoded: bool) -> Shown {
    match protocol {
        Some(_) if !decoded => Shown::Path,
        Some(ProtocolType::Halfblocks) => Shown::Preview,
        Some(_) => Shown::Sharp,
        None => Shown::Path,
    }
}

// herdr answers the Kitty query but not the cell size while no client is attached, and draws Kitty placeholders once one is.
fn choose_protocol(forced: Option<&str>, tty: bool, herdr: bool, answered: Option<ProtocolType>) -> Option<ProtocolType> {
    match forced {
        Some("off") => None,
        Some("halfblocks") => Some(ProtocolType::Halfblocks),
        Some("kitty") => Some(ProtocolType::Kitty),
        Some("sixel") => Some(ProtocolType::Sixel),
        Some("iterm2") => Some(ProtocolType::Iterm2),
        _ if !tty => None,
        _ if herdr && matches!(answered, None | Some(ProtocolType::Halfblocks)) => Some(ProtocolType::Kitty),
        _ => Some(answered.unwrap_or(ProtocolType::Halfblocks)),
    }
}

const GUESSED_CELL: (u16, u16) = (10, 20);

// herdr's plugin popup reports no pixels, so the popup board draws at the cell size a board in a normal pane last measured.
fn cell_file() -> Option<PathBuf> {
    lead::state_dir().ok().map(|state| state.join("cell-size"))
}

fn remember_cell((width, height): (u16, u16)) {
    if let Some(file) = cell_file() && remembered_in(std::fs::read_to_string(&file).ok().as_deref()) != Some((width, height)) {
        let _ = crate::state::write(&file, format!("{width} {height}"));
    }
}

fn remembered_cell() -> Option<(u16, u16)> {
    remembered_in(std::fs::read_to_string(cell_file()?).ok().as_deref())
}

fn remembered_in(text: Option<&str>) -> Option<(u16, u16)> {
    let mut parts = text?.split_whitespace().map(str::parse::<u16>);
    let cell = (parts.next()?.ok()?, parts.next()?.ok()?);
    (cell.0 > 0 && cell.1 > 0).then_some(cell)
}

fn cell_size(answered: Option<(u16, u16)>, window: Option<(u16, u16, u16, u16)>) -> Option<(u16, u16)> {
    let known = |(width, height): (u16, u16)| width > 0 && height > 0;
    answered.filter(|cell| known(*cell)).or_else(|| {
        let (columns, rows, width, height) = window?;
        (columns > 0 && rows > 0).then(|| (width / columns, height / rows)).filter(|cell| known(*cell))
    })
}

fn window_pixels() -> Option<(u16, u16, u16, u16)> {
    let size = ratatui::crossterm::terminal::window_size().ok()?;
    Some((size.columns, size.rows, size.width, size.height))
}

fn picker_for(protocol: ProtocolType, (width, height): (u16, u16)) -> Picker {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(FontSize::new(width, height));
    picker.set_protocol_type(protocol);
    picker
}

// Over SSH a viewer would open on the other machine, so o is offered only where the board sits.
pub(crate) fn image_viewer(os: &str, set: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return None;
    }
    match os {
        "macos" => Some("open"),
        "linux" if set("DISPLAY") || set("WAYLAND_DISPLAY") => Some("xdg-open"),
        _ => None,
    }
}

fn openable(file: &Path) -> bool {
    use image::ImageFormat::{Gif, Jpeg, Png, WebP};
    file.is_file() && matches!(image::ImageFormat::from_path(file), Ok(Png | Jpeg | Gif | WebP))
}

pub(super) fn preview_note(viewer: bool) -> String {
    if viewer { format!("preview · {} larger · {} opens the image", keys::map().label(Act::Fullscreen), keys::map().label(Act::OpenImage)) } else { format!("preview · {} larger", keys::map().label(Act::Fullscreen)) }
}

const IMAGE_BYTES: u64 = 20 * 1024 * 1024;

// A card names whatever file its task chose, so it is decoded here, capped in size, and a decoder that panics costs the image only.
fn decode_image(path: &Path) -> Option<image::DynamicImage> {
    if std::fs::metadata(path).ok()?.len() > IMAGE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader.decode().ok())).ok().flatten()
}

pub(super) type Decoded = (PathBuf, Option<image::DynamicImage>);

pub(super) fn spawn_decoder() -> Option<(Sender<PathBuf>, Receiver<Decoded>)> {
    let (request_tx, request_rx) = mpsc::channel::<PathBuf>();
    let (image_tx, image_rx) = mpsc::channel();
    thread::Builder::new().name("kelpie-images".to_string()).spawn(move || {
        while let Ok(path) = request_rx.recv() {
            let decoded = decode_image(&path);
            if image_tx.send((path, decoded)).is_err() {
                return;
            }
        }
    }).ok()?;
    Some((request_tx, image_rx))
}

// herdr 0.9.5 gives a plugin popup's terminal no pixel size, so a Kitty placement without c/r spans one pixel per cell there; naming its grid in cells places it right in every pane.
pub(super) fn render_image(frame: &mut Frame, protocol: &Protocol, rect: Rect) {
    frame.render_widget(Image::new(protocol), rect);
    let size = protocol.size();
    if let Some(cell) = frame.buffer_mut().cell_mut((rect.x, rect.y)) {
        let symbol = cell.symbol().to_string();
        if let Some(placed) = with_cell_grid(&symbol, size.width, size.height) {
            cell.set_symbol(&placed);
        }
    }
}

fn with_cell_grid(symbol: &str, columns: u16, rows: u16) -> Option<String> {
    (symbol.contains("a=T,U=1,") && !symbol.contains("a=T,U=1,c=")).then(|| symbol.replacen("a=T,U=1,", &format!("a=T,U=1,c={columns},r={rows},"), 1))
}

impl App {
    // None means the caller draws the path instead.
    pub(super) fn image_size(&mut self, path: &Path, area: Size) -> Option<Size> {
        if !path.is_file() {
            return None;
        }
        self.settle_cell();
        if let Graphics::Pending { wanted } = &mut self.graphics {
            *wanted = true;
        }
        let Graphics::Ready(picker) = &self.graphics else { return None };
        let key = (path.to_path_buf(), area.width, area.height);
        if !self.images.contains_key(&key) {
            let Some(decoded) = self.decoded.get(path) else {
                if let Some(requests) = &self.image_requests {
                    let _ = requests.send(path.to_path_buf());
                }
                self.decoded.insert(path.to_path_buf(), None);
                return None;
            };
            let protocol = decoded.clone().and_then(|decoded| picker.new_protocol(decoded, area, Resize::Scale(Some(FilterType::Lanczos3))).ok());
            self.images.insert(key.clone(), protocol);
        }
        self.images.get(&key)?.as_ref().map(|protocol| protocol.size())
    }

    // A board started while herdr had no client attached learns its cell size once the window reports pixels.
    fn settle_cell(&mut self) {
        if !self.cell_guessed {
            return;
        }
        let Graphics::Ready(picker) = &self.graphics else { return };
        if let Some(cell) = cell_size(None, window_pixels()) {
            remember_cell(cell);
            self.graphics = Graphics::Ready(picker_for(picker.protocol_type(), cell));
            self.images.clear();
            self.cell_guessed = false;
        }
    }

    pub(super) fn protocol(&self) -> Option<ProtocolType> {
        match &self.graphics {
            Graphics::Ready(picker) => Some(picker.protocol_type()),
            Graphics::Pending { .. } | Graphics::Off => None,
        }
    }

    pub(super) fn open_image(&mut self, file: &Path) {
        let Some(viewer) = self.viewer else { return };
        if !openable(file) {
            return self.say(format!("{} is not an image file kelpie opens", file.display()));
        }
        match Command::new(viewer).arg(file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(mut child) => {
                thread::spawn(move || child.wait());
            }
            Err(error) => self.say(format!("{viewer}: {error}")),
        }
    }

    pub(super) fn take_images(&mut self) {
        let Some(results) = &self.image_results else { return };
        while let Ok((path, decoded)) = results.try_recv() {
            self.images.retain(|(drawn, _, _), _| *drawn != path);
            self.decoded.insert(path, decoded);
        }
    }
}

// The query waits up to 2 s for an answer, so it never runs inside herdr and elsewhere only once an image is first wanted.
fn asks_terminal(forced: Option<&str>, tty: bool, herdr: bool) -> bool {
    tty && !herdr && !matches!(forced, Some("off" | "halfblocks"))
}

pub(super) struct GraphicsEnv {
    forced: Option<String>,
    tty: bool,
    herdr: bool,
}

impl GraphicsEnv {
    pub(super) fn current() -> Self {
        GraphicsEnv {
            forced: std::env::var("KELPIE_IMAGES").ok(),
            tty: std::io::IsTerminal::is_terminal(&std::io::stdin()),
            herdr: std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "herdr"),
        }
    }

    pub(super) fn startup(&self) -> (Graphics, bool) {
        if asks_terminal(self.forced.as_deref(), self.tty, self.herdr) { (Graphics::Pending { wanted: false }, false) } else { self.resolve(None) }
    }

    pub(super) fn resolve(&self, asked: Option<Picker>) -> (Graphics, bool) {
        resolve_graphics(self.forced.as_deref(), self.tty, self.herdr, asked)
    }
}

fn resolve_graphics(forced: Option<&str>, tty: bool, herdr: bool, asked: Option<Picker>) -> (Graphics, bool) {
    let Some(protocol) = choose_protocol(forced, tty, herdr, asked.as_ref().map(Picker::protocol_type)) else { return (Graphics::Off, false) };
    let answered = asked.as_ref().and_then(|picker| {
        let reported = picker.capabilities().iter().find_map(|capability| match capability { Capability::CellSize(Some(cell)) => Some(*cell), _ => None });
        reported.or_else(|| (picker.protocol_type() != ProtocolType::Halfblocks).then(|| (picker.font_size().width, picker.font_size().height)))
    });
    let cell = cell_size(answered, window_pixels());
    if let Some(cell) = cell {
        remember_cell(cell);
    }
    (Graphics::Ready(picker_for(protocol, cell.or_else(remembered_cell).unwrap_or(GUESSED_CELL))), cell.is_none() && protocol != ProtocolType::Halfblocks)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ratatui::layout::Size;
    use ratatui_image::picker::ProtocolType;

    use super::*;
    use crate::board::test_support::*;
    use crate::board::link::file_url;

    #[test]
    fn a_remembered_cell_size_reads_back_and_garbage_is_none() {
        assert_eq!(remembered_in(Some("16 34")), Some((16, 34)));
        assert_eq!(remembered_in(Some("0 34")), None);
        assert_eq!(remembered_in(Some("x")), None);
        assert_eq!(remembered_in(None), None);
    }

    #[test]
    fn a_kitty_placement_names_its_grid_in_cells_once() {
        let sent = "\x1b_Gq=2,a=T,U=1,f=32,t=d,s=200,v=120;AAAA\x1b\\";
        let placed = with_cell_grid(sent, 20, 6).unwrap();
        assert!(placed.contains("a=T,U=1,c=20,r=6,f=32"), "{placed}");
        assert_eq!(with_cell_grid(&placed, 20, 6), None, "a placement that names its grid is left as it is");
        assert_eq!(with_cell_grid("▀", 20, 6), None, "halfblocks carry no placement");
    }

    #[test]
    fn the_protocol_follows_the_forced_choice_then_the_tty_then_herdr_then_the_answer() {
        use ProtocolType::{Halfblocks, Iterm2, Kitty, Sixel};
        assert_eq!(choose_protocol(Some("off"), true, true, Some(Kitty)), None);
        assert_eq!(choose_protocol(Some("halfblocks"), true, false, Some(Kitty)), Some(Halfblocks));
        assert_eq!(choose_protocol(Some("kitty"), false, false, None), Some(Kitty), "a forced protocol needs no tty");
        assert_eq!(choose_protocol(Some("sixel"), true, false, None), Some(Sixel));
        assert_eq!(choose_protocol(Some("iterm2"), true, false, None), Some(Iterm2));
        assert_eq!(choose_protocol(None, false, true, None), None, "no tty, no picture");
        assert_eq!(choose_protocol(None, true, true, Some(Halfblocks)), Some(Kitty), "herdr draws Kitty placeholders though its query says halfblocks");
        assert_eq!(choose_protocol(None, true, true, None), Some(Kitty), "herdr with no answer at all");
        assert_eq!(choose_protocol(Some("bogus"), true, true, None), Some(Kitty), "an unknown value is detection");
        assert_eq!(choose_protocol(None, true, false, Some(Sixel)), Some(Sixel), "outside herdr the answer decides");
        assert_eq!(choose_protocol(None, true, false, Some(Halfblocks)), Some(Halfblocks));
        assert_eq!(choose_protocol(None, true, false, None), Some(Halfblocks));
    }

    #[test]
    fn the_board_never_asks_the_terminal_inside_herdr_and_elsewhere_only_once_an_image_is_wanted() {
        assert!(!asks_terminal(None, true, true), "herdr: the protocol comes from the environment");
        assert!(!asks_terminal(Some("kitty"), true, true));
        assert!(asks_terminal(None, true, false) && asks_terminal(Some("kitty"), true, false), "elsewhere the answer gives the cell size");
        assert!(!asks_terminal(Some("off"), true, false) && !asks_terminal(Some("halfblocks"), true, false) && !asks_terminal(None, false, false));
        let herdr = GraphicsEnv { forced: None, tty: true, herdr: true }.startup().0;
        assert!(matches!(&herdr, Graphics::Ready(picker) if picker.protocol_type() == ProtocolType::Kitty), "Kitty at startup with no query inside herdr");
        assert!(matches!(GraphicsEnv { forced: None, tty: true, herdr: false }.startup().0, Graphics::Pending { wanted: false }), "the first frame draws before any query");
        let dir = PathBuf::from(format!("/tmp/kq{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).save(&png).unwrap();
        let mut app = test_app(vec![]);
        app.graphics = Graphics::Pending { wanted: false };
        assert_eq!(app.image_size(&dir.join("missing.png"), Size::new(10, 10)), None);
        assert!(matches!(app.graphics, Graphics::Pending { wanted: false }), "a missing file wants no query");
        assert_eq!(app.image_size(&png, Size::new(10, 10)), None, "the path shows until the query between frames has answered");
        assert!(matches!(app.graphics, Graphics::Pending { wanted: true }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_cell_size_is_the_answer_else_the_window_pixels_else_unknown() {
        assert_eq!(cell_size(Some((16, 34)), Some((93, 39, 1488, 1326))), Some((16, 34)));
        assert_eq!(cell_size(None, Some((93, 39, 1488, 1326))), Some((16, 34)), "herdr with a client sets the pane's pixels");
        assert_eq!(cell_size(Some((0, 0)), Some((119, 40, 0, 0))), None, "herdr with no client attached knows no pixels");
        assert_eq!(cell_size(None, None), None);
    }

    #[test]
    fn an_image_is_sharp_with_a_protocol_a_preview_with_halfblocks_and_a_path_otherwise() {
        assert_eq!(image_shown(Some(ProtocolType::Kitty), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Sixel), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Iterm2), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Halfblocks), true), Shown::Preview);
        assert_eq!(image_shown(Some(ProtocolType::Kitty), false), Shown::Path, "an image that fails to decode");
        assert_eq!(image_shown(None, true), Shown::Path, "KELPIE_IMAGES=off or no tty");
    }

    #[test]
    fn o_opens_with_the_system_viewer_where_the_board_sits_and_not_over_ssh() {
        let env = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(image_viewer("macos", &env(&[])), Some("open"));
        assert_eq!(image_viewer("linux", &env(&[])), None, "no display on Linux");
        assert_eq!(image_viewer("linux", &env(&["DISPLAY"])), Some("xdg-open"));
        assert_eq!(image_viewer("linux", &env(&["WAYLAND_DISPLAY"])), Some("xdg-open"));
        assert_eq!(image_viewer("macos", &env(&["SSH_CONNECTION"])), None);
        assert_eq!(image_viewer("linux", &env(&["DISPLAY", "SSH_TTY"])), None);
        assert_eq!(image_viewer("windows", &env(&[])), None);
        assert!(!openable(Path::new("/nowhere/x.png")) && !openable(Path::new("/bin/sh")));
        assert_eq!(file_url(Path::new("/tmp/a b/ö.png")), "file:///tmp/a%20b/%C3%B6.png");
    }

    #[test]
    fn an_image_too_large_or_not_an_image_is_not_decoded() {
        let dir = std::env::temp_dir().join(format!("kelpie-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        assert!(decode_image(&fake).is_none());
        let big = dir.join("big.png");
        std::fs::File::create(&big).unwrap().set_len(IMAGE_BYTES + 1).unwrap();
        assert!(decode_image(&big).is_none());
        assert!(decode_image(&dir.join("missing.png")).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

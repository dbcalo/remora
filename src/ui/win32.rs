//! The Windows window, made of Win32 controls so it is a Windows app, in the
//! machine's light or dark theme (decided 2026-11-07), switching live when Windows'
//! own setting changes.
//!
//! Classic Win32 controls have no supported dark mode, so this does what
//! Notepad++ and Windows' own dialogs do: the documented dark title bar
//! (DwmSetWindowAttribute), Windows' built-in dark control themes
//! (DarkMode_Explorer, DarkMode_CFD: undocumented names, stable and widely used),
//! and colours painted for everything else. Two controls ignore text colour under
//! those themes, group-box captions and checkbox labels, so their text is a
//! separate label. The tab control has no dark theme at all, so the pages are
//! switched by a selector bar, Windows 11 style: names with an accent underline.
//!
//! It paints `view::View` once a second and sends `view::Action`s; it decides
//! nothing. Fixed layout: no control moves; pages swap whole. Tab and Enter work
//! through IsDialogMessage; quitting while something plays asks with Windows' own
//! message box.

use std::cell::{Cell, RefCell};
use std::ptr::null;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmGetColorizationColor, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontIndirectW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawFocusRect, DrawTextW, FillRect, GetSysColor, HBRUSH, HDC, HFONT, InvalidateRect, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_NOPREFIX, SS_NOTIFY};
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, DRAWITEMSTRUCT, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES, ICC_UPDOWN_CLASS,
    INITCOMMONCONTROLSEX, InitCommonControlsEx, ODS_FOCUS, PBM_SETMARQUEE, PBS_MARQUEE, SetWindowTheme, UDM_GETPOS32,
    UDM_SETBUDDY, UDM_SETPOS32, UDM_SETRANGE32, UDS_ALIGNRIGHT, UDS_ARROWKEYS, UDS_SETBUDDYINT,
};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::controller::Controller;
use crate::decide;
use crate::view::{Action, View};

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

// Command ids.
const ID_START_STOP: usize = 101;
const ID_OPEN: usize = 102;
const ID_QUIT: usize = 103;
const ID_PRIVATE: usize = 110;
const ID_ZONE: usize = 111;
const ID_IDLE_ON: usize = 112;
const ID_IDLE_SAVE: usize = 113;
const ID_STEAM: usize = 114;
const ID_ACCOUNT: usize = 115;
const ID_IDLE_ON_LABEL: usize = 116;
const ID_SETUP_PRIVATE: usize = 120;
const ID_SETUP_ZONE: usize = 121;
const ID_SKIP: usize = 122;
const ID_SUBMIT: usize = 123;
const ID_SET_PW: usize = 124;
const ID_USERS_REFRESH: usize = 125;
const ID_USERS_LIST: usize = 126;
const ID_NAV: usize = 130; // one per NAV name: 130 to 134
const ID_SCAN: usize = 140;
const ID_COPY: usize = 141;
const ID_OPEN_ADDR: usize = 142;
const ID_RESTART: usize = 143;
const ID_ADD_FOLDER: usize = 144;
const ID_REMOVE_FOLDER: usize = 145;
const ID_REMOVE_LIBRARY: usize = 146;
const ID_BROWSE: usize = 147;
const ID_ADD_LIBRARY: usize = 148;
const ID_ADD_USER: usize = 149;
const ID_OPEN_LOGS: usize = 150;
const TIMER: usize = 1;

const STATUS: usize = 0;
const LIBRARY: usize = 1;
const USERS: usize = 2;
const SETTINGS: usize = 3;
const ABOUT: usize = 4;
const SETUP: usize = 5;
const PAGES: usize = 6;
/// The selector bar's names, one per page before SETUP.
const NAV: [&str; 5] = ["Status", "Library", "Users", "Settings", "About"];

/// The colours of one theme.
struct Theme {
    dark: bool,
    bg: u32,
    fg: u32,
    dim: u32,
    edit_bg: u32,
    accent: u32,
    bg_brush: HBRUSH,
    edit_brush: HBRUSH,
}

impl Theme {
    fn new(dark: bool) -> Theme {
        // SAFETY: plain GDI/DWM queries.
        unsafe {
            let (bg, fg, dim, edit_bg) = if dark {
                (rgb(32, 32, 32), rgb(255, 255, 255), rgb(197, 197, 197), rgb(45, 45, 45))
            } else {
                (GetSysColor(COLOR_WINDOW), GetSysColor(COLOR_WINDOWTEXT), rgb(96, 96, 96), GetSysColor(COLOR_WINDOW))
            };
            // The accent from the user's colour settings, as Windows 11 uses it.
            let (mut argb, mut opaque) = (0u32, 0i32);
            let accent = if DwmGetColorizationColor(&mut argb, &mut opaque) == 0 {
                rgb((argb >> 16) as u8, (argb >> 8) as u8, argb as u8)
            } else {
                rgb(0, 103, 192)
            };
            Theme { dark, bg, fg, dim, edit_bg, accent, bg_brush: CreateSolidBrush(bg), edit_brush: CreateSolidBrush(edit_bg) }
        }
    }

    /// Windows' setting, read now. JELLYFIN_PORTABLE_THEME=light or dark forces
    /// one, for checking a theme without changing the machine's setting.
    fn current() -> Theme {
        match std::env::var("JELLYFIN_PORTABLE_THEME").as_deref() {
            Ok("light") => return Theme::new(false),
            Ok("dark") => return Theme::new(true),
            _ => {}
        }
        let key = w(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let name = w("AppsUseLightTheme");
        let (mut value, mut size) = (0u32, 4u32);
        // SAFETY: value is a DWORD and size says so.
        let ok = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                &mut value as *mut u32 as *mut _,
                &mut size,
            )
        } == 0;
        Theme::new(decide::dark_theme(ok.then_some(value)))
    }
}

impl Drop for Theme {
    fn drop(&mut self) {
        // SAFETY: brushes this theme created.
        unsafe {
            DeleteObject(self.bg_brush as _);
            DeleteObject(self.edit_brush as _);
        }
    }
}

#[derive(Default)]
struct Controls {
    nav: [HWND; 5],
    phase: HWND,
    start_stop: HWND,
    restart: HWND,
    tv: HWND,
    copy: HWND,
    open_addr: HWND,
    lib_list: HWND,
    lib_buttons: [HWND; 3],
    lib_name: HWND,
    lib_kind: HWND,
    lib_folder: HWND,
    browse: HWND,
    add_library: HWND,
    scan_text: HWND,
    recent: HWND,
    scan: HWND,
    play1: HWND,
    play2: HWND,
    keep: HWND,
    idle: HWND,
    fw_text: HWND,
    fw_private: HWND,
    fw_zone: HWND,
    idle_on: HWND,
    idle_start: HWND,
    idle_start_ud: HWND,
    idle_margin: HWND,
    idle_margin_ud: HWND,
    steam_text: HWND,
    steam_add: HWND,
    account_text: HWND,
    account_btn: HWND,
    setup_title: HWND,
    setup_text: HWND,
    setup_fw_text: HWND,
    setup_private: HWND,
    setup_zone: HWND,
    user: HWND,
    pass: HWND,
    media: HWND,
    media_label: HWND,
    submit: HWND,
    skip: HWND,
    users_text: HWND,
    users_list: HWND,
    new_pw: HWND,
    confirm_pw: HWND,
    set_pw: HWND,
    users_refresh: HWND,
    nu_name: HWND,
    nu_pw: HWND,
    nu_confirm: HWND,
    add_user: HWND,
    progress: HWND,
    message: HWND,
    open: HWND,
    /// Every control, with how to theme it: true for an edit.
    all: Vec<(HWND, bool)>,
}

struct Ui {
    ctrl: Arc<Controller>,
    hwnd: HWND,
    dpi: u32,
    c: Controls,
    pages: [Vec<HWND>; PAGES],
    panels: [HWND; PAGES],
    page: usize,
    showing: usize,
    setup_dismissed: bool,
    /// Seconds left showing "Copied" on the Copy button.
    copied: u8,
    /// Counts timer ticks, to read the scan's progress every few seconds.
    ticks: u32,
    last: Option<View>,
    error: bool,
    theme: Theme,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// What painting needs, copied out of `Ui`. Windows asks for backgrounds and
    /// colours DURING a page switch, while `Ui` is borrowed for it; reading `Ui`
    /// then failed, fell back to Windows' defaults, and left stale, light pixels
    /// (seen in testing). A Cell of a Copy value can always be read.
    static PAINT: Cell<Option<Paint>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Paint {
    bg: u32,
    fg: u32,
    dim: u32,
    edit_bg: u32,
    accent: u32,
    bg_brush: HBRUSH,
    edit_brush: HBRUSH,
    message: HWND,
    tv: HWND,
    error: bool,
    /// The selector bar's selected item, or usize::MAX while setup shows.
    selected: usize,
    dpi: u32,
}

fn paint_state() -> Option<Paint> {
    PAINT.with(Cell::get)
}

/// Copies what painting needs out of `ui`.
fn publish(ui: &Ui) {
    let t = &ui.theme;
    PAINT.with(|p| {
        p.set(Some(Paint {
            bg: t.bg,
            fg: t.fg,
            dim: t.dim,
            edit_bg: t.edit_bg,
            accent: t.accent,
            bg_brush: t.bg_brush,
            edit_brush: t.edit_brush,
            message: ui.c.message,
            tv: ui.c.tv,
            error: ui.error,
            selected: if ui.showing == SETUP { usize::MAX } else { ui.page },
            dpi: ui.dpi,
        }))
    });
}

/// What creating a child control needs: its parent and the DPI to scale by.
#[derive(Clone, Copy)]
struct Geo {
    hwnd: HWND,
    dpi: u32,
    /// The parent's top within the window, in 96-dpi units: layout coordinates
    /// stay window-relative whatever the parent.
    oy: i32,
}

impl Geo {
    fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
}

/// A font from the system message font (Segoe UI), scaled and weighted.
fn font(dpi: u32, scale: f32, weight: i32) -> HFONT {
    let mut ncm: NONCLIENTMETRICSW = unsafe { std::mem::zeroed() };
    ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
    // SAFETY: ncm is sized and zeroed as the call requires.
    unsafe { SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS, ncm.cbSize, &mut ncm as *mut _ as *mut _, 0, dpi) };
    let mut lf = ncm.lfMessageFont;
    lf.lfHeight = (lf.lfHeight as f32 * scale) as i32;
    if weight > 0 {
        lf.lfWeight = weight;
    }
    // SAFETY: lf is a valid LOGFONTW.
    unsafe { CreateFontIndirectW(&lf) }
}

/// Runs the window until it closes.
pub fn run(ctrl: Arc<Controller>) {
    // SAFETY: plain Win32 calls on this thread, which owns the window.
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_UPDOWN_CLASS | ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icc);
        // The folder picker is a COM object; it wants a single-threaded apartment.
        windows_sys::Win32::System::Com::CoInitializeEx(null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32);
        let hinst = GetModuleHandleW(null());
        let class = w("RemoraWindow");
        let icon = LoadIconW(hinst, 1 as _);
        let small = LoadImageW(hinst, 1 as _, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: icon,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(), // painted in WM_ERASEBKGND, per theme
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: small as _,
        };
        RegisterClassExW(&wc);
        let page_class = w(PAGE_CLASS);
        let pc = WNDCLASSEXW { lpfnWndProc: Some(page_proc), lpszClassName: page_class.as_ptr(), hIcon: std::ptr::null_mut(), hIconSm: std::ptr::null_mut(), ..wc };
        RegisterClassExW(&pc);

        let dpi = GetDpiForSystem();
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut r = RECT { left: 0, top: 0, right: 600 * dpi as i32 / 96, bottom: 512 * dpi as i32 / 96 };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
        let title = w(decide::APP_NAME);
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.right - r.left,
            r.bottom - r.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut ui = Ui {
            ctrl,
            hwnd,
            dpi: GetDpiForWindow(hwnd),
            c: Controls::default(),
            pages: Default::default(),
            panels: [std::ptr::null_mut(); PAGES],
            page: STATUS,
            showing: usize::MAX,
            setup_dismissed: false,
            copied: 0,
            ticks: 0,
            last: None,
            error: false,
            theme: Theme::current(),
        };
        build(&mut ui);
        apply_theme(&ui);
        UI.with(|cell| *cell.borrow_mut() = Some(ui));
        UI.with(|cell| {
            if let Some(ui) = cell.borrow_mut().as_mut() {
                paint(ui);
            }
        });
        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, TIMER, 1000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        UI.with(|cell| cell.borrow_mut().take());
    }
}

/// A message box that says only what went wrong, for errors before the window
/// can work.
pub fn error_window(message: &str) {
    let (text, title) = (w(message), w(decide::APP_NAME));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR | MB_OK) };
}

/// Creates one child control at 96-dpi coordinates, scaled, with the font.
unsafe fn child(g: Geo, class: &str, text: &str, style: u32, x: i32, y: i32, cx: i32, cy: i32, id: usize, f: HFONT) -> HWND {
    let is_edit = class == "EDIT";
    let (class, text) = (w(class), w(text));
    // SAFETY: the parent window exists; the strings outlive the call.
    unsafe {
        let h = CreateWindowExW(
            if is_edit { WS_EX_CLIENTEDGE } else { 0 },
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | style,
            g.s(x),
            g.s(y - g.oy),
            g.s(cx),
            g.s(cy),
            g.hwnd,
            id as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(h, WM_SETFONT, f as WPARAM, 1);
        h
    }
}

/// A section heading, Windows 11 Settings style: a bold label, no frame. Group
/// boxes were tried and dropped: under the dark control theme they fill their
/// inside white and draw their caption black.
unsafe fn group(g: Geo, page: &mut Vec<HWND>, caption: &str, x: i32, y: i32, cx: i32, _cy: i32, _f: HFONT, bold: HFONT) {
    // SAFETY: as for child().
    unsafe {
        page.push(child(g, "STATIC", caption, WS_VISIBLE | SS_NOPREFIX, x, y, cx, 20, 0, bold));
    }
}

fn build(ui: &mut Ui) {
    let body = font(ui.dpi, 1.0, 0);
    let bold = font(ui.dpi, 1.0, 600);
    let big = font(ui.dpi, 1.6, 600);
    let title = font(ui.dpi, 2.0, 600);
    let label = WS_VISIBLE | SS_NOPREFIX;
    let one_line = label | SS_ENDELLIPSIS;
    let button = WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32;
    let edit = WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32;
    let gm = Geo { hwnd: ui.hwnd, dpi: ui.dpi, oy: 0 };
    let g = gm;

    // SAFETY: every call below creates a child of ui.hwnd on this thread.
    unsafe {
        let mut p: [Vec<HWND>; PAGES] = Default::default();
        // One container per page, below the selector bar (setup's covers the
        // bar too, which is hidden then). Switching pages shows one container
        // and hides another, and Windows repaints what they uncover: hiding
        // dozens of loose controls left their pixels on screen.
        let panel = |top: i32, height: i32| {
            let class = w(PAGE_CLASS);
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class.as_ptr(),
                null(),
                WS_CHILD | WS_CLIPCHILDREN,
                0,
                gm.s(top),
                gm.s(600),
                gm.s(height),
                ui.hwnd,
                std::ptr::null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        ui.panels = [panel(50, 406), panel(50, 406), panel(50, 406), panel(50, 406), panel(50, 406), panel(0, 456)];
        let gp = ui.panels.map(|h| Geo { hwnd: h, dpi: ui.dpi, oy: if h == ui.panels[SETUP] { 0 } else { 50 } });
        let c = &mut ui.c;

        // The selector bar: page names drawn by us (WM_DRAWITEM), so they follow
        // the theme; the selected one has an accent underline.
        for (i, name) in NAV.iter().enumerate() {
            c.nav[i] = child(g, "BUTTON", name, WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32, 16 + i as i32 * 96, 10, 90, 34, ID_NAV + i, body);
        }

        // --- Status ---
        let s = &mut p[STATUS];
        let g = gp[STATUS];
        group(g, s, "Server", 20, 56, 560, 60, body, bold);
        c.phase = child(g, "STATIC", "", one_line, 36, 84, 300, 20, 0, body);
        c.restart = child(g, "BUTTON", "Restart", button, 340, 78, 108, 28, ID_RESTART, body);
        c.start_stop = child(g, "BUTTON", "Start server", button, 456, 78, 108, 28, ID_START_STOP, body);
        group(g, s, "Server address", 20, 124, 560, 72, body, bold);
        c.tv = child(g, "EDIT", "", edit | ES_READONLY as u32, 36, 152, 360, 34, 0, big);
        c.copy = child(g, "BUTTON", "Copy", button, 404, 154, 76, 30, ID_COPY, body);
        c.open_addr = child(g, "BUTTON", "Open", button, 488, 154, 76, 30, ID_OPEN_ADDR, body);
        group(g, s, "Now playing", 20, 204, 560, 76, body, bold);
        c.play1 = child(g, "STATIC", "", one_line, 36, 230, 528, 20, 0, body);
        c.play2 = child(g, "STATIC", "", one_line, 36, 252, 528, 20, 0, body);
        s.push(child(g, "STATIC", "Keep-awake", label, 20, 294, 104, 20, 0, bold));
        c.keep = child(g, "STATIC", "", label, 128, 294, 452, 40, 0, body);
        s.push(child(g, "STATIC", "Auto-stop", label, 20, 342, 104, 20, 0, bold));
        c.idle = child(g, "STATIC", "", label, 128, 342, 452, 40, 0, body);
        let logs = child(g, "BUTTON", "Open logs folder", button, 20, 396, 140, 28, ID_OPEN_LOGS, body);
        s.push(logs);
        s.extend([c.phase, c.restart, c.start_stop, c.tv, c.copy, c.open_addr, c.play1, c.play2, c.keep, c.idle]);

        // --- Library ---
        let s = &mut p[LIBRARY];
        let g = gp[LIBRARY];
        group(g, s, "Libraries", 20, 56, 560, 134, body, bold);
        c.lib_list = child(
            g,
            "LISTBOX",
            "",
            WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            36,
            82,
            404,
            108,
            0,
            body,
        );
        c.lib_buttons = [
            child(g, "BUTTON", "Add folder\u{2026}", button, 452, 82, 112, 28, ID_ADD_FOLDER, body),
            child(g, "BUTTON", "Remove folder", button, 452, 116, 112, 28, ID_REMOVE_FOLDER, body),
            child(g, "BUTTON", "Remove library", button, 452, 150, 112, 28, ID_REMOVE_LIBRARY, body),
        ];
        group(g, s, "New library", 20, 198, 560, 120, body, bold);
        s.push(child(g, "STATIC", "&Name", label & !SS_NOPREFIX, 36, 220, 200, 18, 0, body));
        c.lib_name = child(g, "EDIT", "", edit, 36, 238, 200, 24, 0, body);
        s.push(child(g, "STATIC", "&Type", label & !SS_NOPREFIX, 248, 220, 192, 18, 0, body));
        c.lib_kind = child(g, "COMBOBOX", "", WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32, 248, 238, 192, 200, 0, body);
        s.push(child(g, "STATIC", "&Folder", label & !SS_NOPREFIX, 36, 268, 200, 18, 0, body));
        c.lib_folder = child(g, "EDIT", "", edit, 36, 286, 300, 24, 0, body);
        c.browse = child(g, "BUTTON", "Browse\u{2026}", button, 344, 285, 96, 26, ID_BROWSE, body);
        c.add_library = child(g, "BUTTON", "Add library", button, 452, 285, 112, 26, ID_ADD_LIBRARY, body);
        for (shown, _) in decide::LIBRARY_KINDS {
            let t = w(shown);
            SendMessageW(c.lib_kind, CB_ADDSTRING, 0, t.as_ptr() as LPARAM);
        }
        SendMessageW(c.lib_kind, CB_SETCURSEL, 0, 0);
        group(g, s, "Scan", 20, 322, 560, 60, body, bold);
        c.scan_text = child(g, "STATIC", "", one_line, 36, 348, 396, 20, 0, body);
        c.scan = child(g, "BUTTON", "Scan all libraries", button, 432, 342, 132, 28, ID_SCAN, body);
        group(g, s, "Recently added", 20, 382, 560, 50, body, bold);
        c.recent = child(g, "STATIC", "", one_line, 36, 406, 528, 20, 0, body);
        s.extend([c.recent, c.lib_list, c.lib_name, c.lib_kind, c.lib_folder, c.browse, c.add_library, c.scan_text, c.scan]);
        s.extend(c.lib_buttons);

        // --- Settings ---
        let s = &mut p[SETTINGS];
        let g = gp[SETTINGS];
        group(g, s, "Network", 20, 56, 560, 96, body, bold);
        c.fw_text = child(g, "STATIC", "", label, 36, 82, 528, 34, 0, body);
        c.fw_private = child(g, "BUTTON", "Mark this network Private", button, 36, 118, 190, 26, ID_PRIVATE, body);
        c.fw_zone = child(g, "BUTTON", "Open the ports in this zone", button, 234, 118, 190, 26, ID_ZONE, body);
        group(g, s, "Auto-stop", 20, 160, 560, 116, body, bold);
        // A checkbox without text, and its own label: themed checkboxes draw
        // their text black whatever the theme. Clicking the label toggles it.
        c.idle_on = child(g, "BUTTON", "", WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32, 36, 186, 18, 20, ID_IDLE_ON, body);
        s.push(child(g, "STATIC", "Stop the server when it goes unused", label | SS_NOTIFY, 58, 187, 380, 20, ID_IDLE_ON_LABEL, body));
        s.push(child(g, "STATIC", "Minutes to start playing", label, 36, 218, 240, 20, 0, body));
        c.idle_start = child(g, "EDIT", "", edit | ES_NUMBER as u32, 290, 214, 64, 24, 0, body);
        s.push(child(g, "STATIC", "Minutes after the end of what played", label, 36, 246, 240, 20, 0, body));
        c.idle_margin = child(g, "EDIT", "", edit | ES_NUMBER as u32, 290, 242, 64, 24, 0, body);
        c.idle_start_ud = updown(g, c.idle_start);
        c.idle_margin_ud = updown(g, c.idle_margin);
        let save = child(g, "BUTTON", "Save", button, 456, 240, 108, 26, ID_IDLE_SAVE, body);
        group(g, s, "Steam", 20, 284, 560, 70, body, bold);
        c.steam_text = child(g, "STATIC", "", label, 36, 310, 404, 36, 0, body);
        c.steam_add = child(g, "BUTTON", "Add to Steam", button, 456, 312, 108, 26, ID_STEAM, body);
        group(g, s, "Account", 20, 362, 560, 70, body, bold);
        c.account_text = child(g, "STATIC", "", label, 36, 388, 404, 36, 0, body);
        c.account_btn = child(g, "BUTTON", "Sign in again", button, 456, 390, 108, 26, ID_ACCOUNT, body);
        s.extend([c.fw_text, c.fw_private, c.fw_zone, c.idle_on, c.idle_start, c.idle_start_ud, c.idle_margin, c.idle_margin_ud, save]);
        s.extend([c.steam_text, c.steam_add, c.account_text, c.account_btn]);

        // --- Users ---
        let s = &mut p[USERS];
        let g = gp[USERS];
        group(g, s, "Users", 20, 56, 560, 160, body, bold);
        c.users_text = child(g, "STATIC", "", label, 36, 82, 528, 36, 0, body);
        c.users_list = child(
            g,
            "LISTBOX",
            "",
            WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            36,
            122,
            404,
            96,
            ID_USERS_LIST,
            body,
        );
        c.users_refresh = child(g, "BUTTON", "Refresh", button, 452, 122, 112, 28, ID_USERS_REFRESH, body);
        group(g, s, "New password for the chosen user", 20, 228, 560, 90, body, bold);
        s.push(child(g, "STATIC", "&New password", label & !SS_NOPREFIX, 36, 250, 200, 18, 0, body));
        c.new_pw = child(g, "EDIT", "", edit | ES_PASSWORD as u32, 36, 268, 200, 24, 0, body);
        s.push(child(g, "STATIC", "&Confirm", label & !SS_NOPREFIX, 244, 250, 196, 18, 0, body));
        c.confirm_pw = child(g, "EDIT", "", edit | ES_PASSWORD as u32, 244, 268, 196, 24, 0, body);
        c.set_pw = child(g, "BUTTON", "Set password", button, 452, 267, 112, 26, ID_SET_PW, body);
        s.push(child(g, "STATIC", "Both blank removes the password; an administrator always keeps one.", one_line, 36, 298, 528, 20, 0, body));
        group(g, s, "New user", 20, 326, 560, 80, body, bold);
        s.push(child(g, "STATIC", "Na&me", label & !SS_NOPREFIX, 36, 348, 140, 18, 0, body));
        c.nu_name = child(g, "EDIT", "", edit, 36, 366, 140, 24, 0, body);
        s.push(child(g, "STATIC", "Pass&word", label & !SS_NOPREFIX, 184, 348, 124, 18, 0, body));
        c.nu_pw = child(g, "EDIT", "", edit | ES_PASSWORD as u32, 184, 366, 124, 24, 0, body);
        s.push(child(g, "STATIC", "Con&firm", label & !SS_NOPREFIX, 316, 348, 124, 18, 0, body));
        c.nu_confirm = child(g, "EDIT", "", edit | ES_PASSWORD as u32, 316, 366, 124, 24, 0, body);
        c.add_user = child(g, "BUTTON", "Add user", button, 452, 365, 112, 26, ID_ADD_USER, body);
        s.extend([c.users_text, c.users_list, c.users_refresh, c.new_pw, c.confirm_pw, c.set_pw]);
        s.extend([c.nu_name, c.nu_pw, c.nu_confirm, c.add_user]);

        // --- About --- (Win32 needs no toolkit attribution)
        let s = &mut p[ABOUT];
        let g = gp[ABOUT];
        s.push(child(g, "STATIC", decide::APP_NAME, label, 20, 60, 520, 36, 0, title));
        s.push(child(
            g,
            "STATIC",
            "Remora runs a portable Jellyfin server from this folder, shows the address to type into the TV, and keeps \
             this machine awake while something plays. Closing the window stops the server.",
            label,
            20,
            104,
            540,
            60,
            0,
            body,
        ));
        s.push(child(g, "STATIC", "Remora is not made by or affiliated with the Jellyfin project.", label, 20, 172, 540, 24, 0, body));

        // --- Setup --- (replaces the pages while there is no working key)
        let s = &mut p[SETUP];
        let g = gp[SETUP];
        c.setup_title = child(g, "STATIC", "", label, 20, 16, 540, 36, 0, title);
        c.setup_text = child(g, "STATIC", "", label, 20, 56, 560, 40, 0, body);
        group(g, s, "1. Network", 20, 100, 560, 88, body, bold);
        c.setup_fw_text = child(g, "STATIC", "", label, 36, 124, 528, 32, 0, body);
        c.setup_private = child(g, "BUTTON", "Mark this network Private", button, 36, 156, 190, 26, ID_SETUP_PRIVATE, body);
        c.setup_zone = child(g, "BUTTON", "Open the ports in this zone", button, 234, 156, 190, 26, ID_SETUP_ZONE, body);
        group(g, s, "2. Administrator", 20, 196, 560, 200, body, bold);
        s.push(child(g, "STATIC", "&Name", label & !SS_NOPREFIX, 36, 222, 300, 18, 0, body));
        c.user = child(g, "EDIT", "", edit, 36, 240, 528, 24, 0, body);
        s.push(child(g, "STATIC", "&Password", label & !SS_NOPREFIX, 36, 272, 300, 18, 0, body));
        c.pass = child(g, "EDIT", "", edit | ES_PASSWORD as u32, 36, 290, 528, 24, 0, body);
        c.media_label = child(g, "STATIC", "&Media folder", label & !SS_NOPREFIX, 36, 322, 400, 18, 0, body);
        c.media = child(g, "EDIT", "", edit, 36, 340, 528, 24, 0, body);
        c.skip = child(g, "BUTTON", "Skip for now", button, 346, 410, 110, 30, ID_SKIP, body);
        c.submit = child(g, "BUTTON", "Sign in", WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32, 464, 410, 116, 30, ID_SUBMIT, body);
        s.extend([c.setup_title, c.setup_text, c.setup_fw_text, c.setup_private, c.setup_zone]);
        s.extend([c.user, c.pass, c.media_label, c.media, c.skip, c.submit]);

        // --- always shown: progress, message, Open Jellyfin, Quit ---
        let g = gm;
        c.progress = child(g, "msctls_progress32", "", PBS_MARQUEE, 20, 470, 60, 14, 0, body);
        SendMessageW(c.progress, PBM_SETMARQUEE, 1, 30);
        c.message = child(g, "STATIC", "", label, 88, 464, 290, 34, 0, body);
        c.open = child(g, "BUTTON", "Open Jellyfin", button, 386, 466, 110, 30, ID_OPEN, body);
        let quit = child(g, "BUTTON", "Quit", button, 504, 466, 76, 30, ID_QUIT, body);

        // Which theme each control takes: edits one kind, the rest the other.
        let edits = [c.tv, c.idle_start, c.idle_margin, c.user, c.pass, c.media, c.users_list, c.new_pw, c.confirm_pw, c.nu_name, c.nu_pw, c.nu_confirm, c.lib_list, c.lib_name, c.lib_kind, c.lib_folder];
        let mut all: Vec<(HWND, bool)> = p.iter().flatten().map(|&h| (h, edits.contains(&h))).collect();
        all.extend([c.open, quit, c.message, c.progress].map(|h| (h, false)));
        all.extend(c.nav.map(|h| (h, false)));
        c.all = all;
        ui.pages = p;
    }
    let (on, start, margin) = ui.ctrl.idle_settings();
    // SAFETY: the controls were created above.
    unsafe {
        SendMessageW(ui.c.idle_on, BM_SETCHECK, if on { BST_CHECKED as usize } else { 0 }, 0);
        SendMessageW(ui.c.idle_start_ud, UDM_SETPOS32, 0, start as LPARAM);
        SendMessageW(ui.c.idle_margin_ud, UDM_SETPOS32, 0, margin as LPARAM);
    }
}

/// A spin button attached to an edit.
unsafe fn updown(g: Geo, buddy: HWND) -> HWND {
    let class = w("msctls_updown32");
    // SAFETY: as for child().
    unsafe {
        let h = CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE | (UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS) as u32,
            0,
            0,
            0,
            0,
            g.hwnd,
            std::ptr::null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(h, UDM_SETBUDDY, buddy as WPARAM, 0);
        SendMessageW(h, UDM_SETRANGE32, 5, 600);
        h
    }
}

/// Puts the current theme on the title bar and every control, and repaints.
fn apply_theme(ui: &Ui) {
    publish(ui);
    let dark = ui.theme.dark as i32;
    let (button, edit) = if ui.theme.dark { (w("DarkMode_Explorer"), w("DarkMode_CFD")) } else { (w("Explorer"), w("CFD")) };
    // SAFETY: our own window and controls; the strings outlive the calls.
    unsafe {
        DwmSetWindowAttribute(ui.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as *const _, 4);
        for &(h, is_edit) in &ui.c.all {
            SetWindowTheme(h, if is_edit { edit.as_ptr() } else { button.as_ptr() }, null());
        }
        RedrawWindow(ui.hwnd, null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
    }
}

fn set_text(h: HWND, s: &str) {
    let t = w(s);
    // SAFETY: h is one of our controls; t is NUL-terminated.
    unsafe { SetWindowTextW(h, t.as_ptr()) };
}

fn get_text(h: HWND) -> String {
    // SAFETY: the buffer is sized from the control's own length.
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

/// The control with the keyboard focus.
fn focused() -> HWND {
    // SAFETY: a plain query on this thread.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() }
}

fn enable(h: HWND, on: bool) {
    // SAFETY: h is one of our controls.
    unsafe { EnableWindow(h, on as i32) };
}

/// Shows exactly one page.
fn show(ui: &mut Ui, page: usize) {
    if ui.showing == page {
        return;
    }
    ui.showing = page;
    publish(ui);
    // SAFETY: all handles are our controls.
    unsafe {
        for &h in &ui.c.nav {
            ShowWindow(h, if page == SETUP { SW_HIDE } else { SW_SHOW });
            InvalidateRect(h, null(), 1);
        }
        // Hide the old container before showing the new one, so the two never
        // draw over each other.
        for (i, &panel) in ui.panels.iter().enumerate() {
            if i != page {
                ShowWindow(panel, SW_HIDE);
            }
        }
        ShowWindow(ui.panels[page], SW_SHOW);
        if page == SETUP {
            SetFocus(ui.c.user);
        }
    }
}

/// Paints the latest view, touching only what changed.
fn paint(ui: &mut Ui) {
    // SAFETY: plain window queries on our own window.
    let visible = unsafe { IsWindowVisible(ui.hwnd) != 0 && IsIconic(ui.hwnd) == 0 };
    let v = ui.ctrl.view(visible);
    let page = if v.setup_wanted && !ui.setup_dismissed { SETUP } else { ui.page };
    show(ui, page);
    if ui.last.as_ref() == Some(&v) {
        return;
    }
    let c = &ui.c;
    let texts: [(HWND, &str); 17] = [
        (c.users_text, &v.users_note),
        (c.scan_text, &v.scan),
        (c.recent, &v.recent),
        (c.phase, &v.phase),
        (c.start_stop, &v.start_stop_label),
        (c.tv, &v.tv),
        (c.play1, &v.play1),
        (c.play2, &v.play2),
        (c.keep, &v.keep),
        (c.idle, &v.idle_stop),
        (c.fw_text, &v.firewall),
        (c.setup_fw_text, &v.firewall),
        (c.steam_text, &v.steam),
        (c.account_text, &v.account),
        (c.account_btn, &v.account_button),
        (c.submit, &v.setup_button),
        (c.message, &v.message),
    ];
    for (h, t) in texts {
        if get_text(h) != t {
            set_text(h, t);
        }
    }
    set_text(c.setup_title, if v.fresh { "Set up Jellyfin" } else { "Sign in to Jellyfin" });
    set_text(
        c.setup_text,
        if v.fresh {
            "This Jellyfin is new. Choose its administrator and the folder with your media. Remote access stays off."
        } else {
            "To see what is playing, this program makes its own API key. Enter the Jellyfin administrator's name and \
             password once; the password is not stored."
        },
    );
    enable(c.start_stop, !v.busy);
    enable(c.open, v.running);
    enable(c.fw_private, v.can_mark_private);
    enable(c.fw_zone, v.can_open_zone);
    enable(c.setup_private, v.can_mark_private);
    enable(c.setup_zone, v.can_open_zone);
    enable(c.steam_add, v.can_add_to_steam);
    enable(c.account_btn, v.running && !v.busy);
    enable(c.media, v.fresh);
    enable(c.media_label, v.fresh);
    enable(c.submit, v.running && !v.busy);
    enable(c.skip, !v.busy);
    for h in [c.users_list, c.users_refresh, c.new_pw, c.confirm_pw, c.set_pw, c.nu_name, c.nu_pw, c.nu_confirm, c.add_user] {
        enable(h, v.can_manage_users);
    }
    if ui.last.as_ref().map(|l| &l.users) != Some(&v.users) {
        fill_users(c.users_list, &v, ui.last.as_ref());
    }
    if ui.last.as_ref().map(|l| &l.libraries) != Some(&v.libraries) {
        fill_libraries(c.lib_list, &v, ui.last.as_ref());
    }
    enable(c.lib_list, v.running);
    for h in c.lib_buttons.into_iter().chain([c.lib_name, c.lib_kind, c.lib_folder, c.browse, c.add_library]) {
        enable(h, v.can_edit_libraries);
    }
    enable(c.scan, v.can_scan);
    enable(c.restart, v.can_restart);
    enable(c.copy, v.running);
    enable(c.open_addr, v.running);
    // SAFETY: our own controls.
    unsafe {
        ShowWindow(c.progress, if v.busy { SW_SHOW } else { SW_HIDE });
        let on = SendMessageW(c.idle_on, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
        for h in [c.idle_start, c.idle_start_ud, c.idle_margin, c.idle_margin_ud] {
            EnableWindow(h, on as i32);
        }
        InvalidateRect(c.message, null(), 1);
    }
    ui.error = v.message_is_error;
    publish(ui);
    ui.last = Some(v);
}

/// Refills the user list, keeping the selected user selected by id.
fn fill_users(list: HWND, v: &View, last: Option<&View>) {
    // SAFETY: our own list box; each string outlives its call.
    unsafe {
        let sel = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let kept = last.and_then(|l| usize::try_from(sel).ok().and_then(|i| l.user_ids.get(i))).cloned();
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        for name in &v.users {
            let t = w(name);
            SendMessageW(list, LB_ADDSTRING, 0, t.as_ptr() as LPARAM);
        }
        if let Some(i) = kept.and_then(|id| v.user_ids.iter().position(|x| *x == id)) {
            SendMessageW(list, LB_SETCURSEL, i, 0);
        }
    }
}

/// Refills the library list, keeping the chosen row chosen.
fn fill_libraries(list: HWND, v: &View, last: Option<&View>) {
    // SAFETY: our own list box; each string outlives its call.
    unsafe {
        let sel = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let kept = last.and_then(|l| usize::try_from(sel).ok().and_then(|i| l.library_rows.get(i))).cloned();
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        for line in &v.libraries {
            let t = w(line);
            SendMessageW(list, LB_ADDSTRING, 0, t.as_ptr() as LPARAM);
        }
        if let Some(i) = kept.and_then(|k| v.library_rows.iter().position(|r| *r == k)) {
            SendMessageW(list, LB_SETCURSEL, i, 0);
        }
    }
}

/// The (library, folder) of the chosen row, if any.
fn selected_library(ui: &Ui) -> Option<(String, String)> {
    // SAFETY: our own list box.
    let i = unsafe { SendMessageW(ui.c.lib_list, LB_GETCURSEL, 0, 0) };
    usize::try_from(i).ok().and_then(|i| ui.last.as_ref()?.library_rows.get(i).cloned())
}

/// Windows' own "Select Folder" dialog (IFileOpenDialog with FOS_PICKFOLDERS),
/// called through its vtable: windows-sys carries no COM interfaces. Modal: call
/// it outside any borrow of the UI. None if cancelled.
fn pick_folder(owner: HWND, title: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows_sys::core::GUID;
    const CLSID_FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);
    const IID_IFILE_OPEN_DIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
    const FOS_PICKFOLDERS: u32 = 0x20;
    const FOS_FORCEFILESYSTEM: u32 = 0x40;
    const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
    // Vtable slots: IUnknown 0-2, IModalWindow::Show 3, then IFileDialog in order.
    const RELEASE: usize = 2;
    const SHOW: usize = 3;
    const SET_OPTIONS: usize = 9;
    const GET_OPTIONS: usize = 10;
    const SET_TITLE: usize = 17;
    const GET_RESULT: usize = 20;
    const GET_DISPLAY_NAME: usize = 5; // IShellItem
    // SAFETY: each slot is called with the signature the Windows SDK gives it,
    // on an object CoCreateInstance returned for exactly that interface; every
    // reference taken is released, and the path string is freed as documented.
    unsafe {
        let slot = |obj: *mut c_void, i: usize| *(*(obj as *const *const usize)).add(i);
        let mut dlg: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_FILE_OPEN_DIALOG, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_IFILE_OPEN_DIALOG, &mut dlg) != 0 {
            return None;
        }
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(slot(dlg, RELEASE));
        let get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = std::mem::transmute(slot(dlg, GET_OPTIONS));
        let set_options: unsafe extern "system" fn(*mut c_void, u32) -> i32 = std::mem::transmute(slot(dlg, SET_OPTIONS));
        let set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 = std::mem::transmute(slot(dlg, SET_TITLE));
        let show: unsafe extern "system" fn(*mut c_void, HWND) -> i32 = std::mem::transmute(slot(dlg, SHOW));
        let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = std::mem::transmute(slot(dlg, GET_RESULT));
        let mut opts = 0u32;
        get_options(dlg, &mut opts);
        set_options(dlg, opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let t = w(title);
        set_title(dlg, t.as_ptr());
        let mut out = None;
        let mut item: *mut c_void = std::ptr::null_mut();
        if show(dlg, owner) == 0 && get_result(dlg, &mut item) == 0 && !item.is_null() {
            let name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32 =
                std::mem::transmute(slot(item, GET_DISPLAY_NAME));
            let mut p: *mut u16 = std::ptr::null_mut();
            if name(item, SIGDN_FILESYSPATH, &mut p) == 0 && !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                CoTaskMemFree(p as *const c_void);
            }
            let item_release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(slot(item, RELEASE));
            item_release(item);
        }
        release(dlg);
        out
    }
}

/// Puts text on the clipboard as Unicode text. False if the clipboard was busy.
fn copy_text(owner: HWND, text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let t = w(text);
    // SAFETY: the memory is sized for t and handed to the clipboard, which then
    // owns it; it is freed here only if the clipboard did not take it.
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, t.len() * 2);
        let mut ok = false;
        if !mem.is_null() {
            let dst = GlobalLock(mem) as *mut u16;
            if !dst.is_null() {
                std::ptr::copy_nonoverlapping(t.as_ptr(), dst, t.len());
                GlobalUnlock(mem);
                ok = !SetClipboardData(CF_UNICODETEXT, mem as _).is_null();
            }
            if !ok {
                GlobalFree(mem);
            }
        }
        CloseClipboard();
        ok
    }
}

/// The id of the user selected in the list, if any.
fn selected_user(ui: &Ui) -> Option<String> {
    // SAFETY: our own list box.
    let i = unsafe { SendMessageW(ui.c.users_list, LB_GETCURSEL, 0, 0) };
    usize::try_from(i).ok().and_then(|i| ui.last.as_ref()?.user_ids.get(i).cloned())
}

/// What a click asks for, decided while the UI is borrowed; carried out after.
enum Do {
    Nothing,
    Act(Action),
    Close,
    /// Asks with a Yes/No message box first; acts on Yes.
    Confirm(String, Action),
    /// Picks a folder, then adds it to this library.
    AddFolder(String),
    /// Picks a folder for the new library's Folder box.
    Browse,
}

fn on_command(ui: &mut Ui, id: usize) -> Do {
    let c = &ui.c;
    match id {
        ID_START_STOP => Do::Act(Action::StartStop),
        ID_OPEN => Do::Act(Action::OpenJellyfin),
        ID_QUIT => Do::Close,
        ID_PRIVATE | ID_SETUP_PRIVATE => Do::Act(Action::MarkPrivate),
        ID_ZONE | ID_SETUP_ZONE => Do::Act(Action::OpenZone),
        ID_STEAM => Do::Act(Action::AddToSteam),
        ID_ACCOUNT => {
            ui.setup_dismissed = false;
            Do::Act(Action::SignInAgain)
        }
        ID_SKIP => {
            ui.setup_dismissed = true;
            Do::Nothing
        }
        _ if (ID_NAV..ID_NAV + NAV.len()).contains(&id) => {
            ui.page = id - ID_NAV;
            publish(ui);
            ui.setup_dismissed = true;
            show(ui, ui.page);
            // Each list is read when its page opens, not on every poll.
            match ui.page {
                USERS => Do::Act(Action::LoadUsers),
                LIBRARY => Do::Act(Action::LoadLibrary),
                _ => Do::Nothing,
            }
        }
        ID_SCAN => Do::Act(Action::Scan),
        ID_ADD_FOLDER => match selected_library(ui) {
            Some((library, _)) => Do::AddFolder(library),
            None => Do::Act(Action::AddFolder { library: String::new(), folder: String::new() }),
        },
        ID_REMOVE_FOLDER => match selected_library(ui) {
            Some((library, folder)) => Do::Confirm(
                format!(
                    "Remove {folder} from {library}?\n\nJellyfin forgets what it knew about the files there, including \
                     what has been watched. The files themselves stay where they are."
                ),
                Action::RemoveFolder { library, folder },
            ),
            None => Do::Act(Action::RemoveFolder { library: String::new(), folder: String::new() }),
        },
        ID_REMOVE_LIBRARY => match selected_library(ui) {
            Some((library, _)) => Do::Confirm(
                format!(
                    "Remove the library {library}?\n\nJellyfin forgets what it knew about its files, including what \
                     has been watched. The files themselves stay where they are."
                ),
                Action::RemoveLibrary { library },
            ),
            None => Do::Act(Action::RemoveLibrary { library: String::new() }),
        },
        ID_BROWSE => Do::Browse,
        ID_ADD_LIBRARY => {
            // SAFETY: our own combo box.
            let i = unsafe { SendMessageW(c.lib_kind, CB_GETCURSEL, 0, 0) };
            let kind = usize::try_from(i).ok().and_then(|i| decide::LIBRARY_KINDS.get(i)).map_or("mixed", |k| k.1);
            Do::Act(Action::AddLibrary { name: get_text(c.lib_name), kind: kind.into(), folder: get_text(c.lib_folder) })
        }
        // No question even while something plays: the TV rides out the few
        // seconds the server is down (tested 2026-11-07: it does not even pause).
        ID_RESTART => Do::Act(Action::Restart),
        ID_OPEN_ADDR => Do::Act(Action::OpenAddress),
        ID_OPEN_LOGS => Do::Act(Action::OpenLogs),
        ID_COPY => {
            if copy_text(ui.hwnd, &get_text(c.tv)) {
                set_text(c.copy, "Copied");
                ui.copied = 2;
            }
            Do::Nothing
        }
        ID_USERS_REFRESH => Do::Act(Action::LoadUsers),
        // The two forms on the Users page, by button or by Enter (IDOK). Enter
        // acts only from its own boxes (`view::users_enter`): from the list it
        // would send a blank SetPassword and remove the user's password.
        ID_ADD_USER | ID_SET_PW | 1 if ui.showing == USERS => {
            use crate::view::{UsersButton, UsersEnter, UsersFocus};
            let f = focused();
            let focus = if [c.nu_name, c.nu_pw, c.nu_confirm].contains(&f) {
                UsersFocus::NewUser
            } else if [c.new_pw, c.confirm_pw].contains(&f) {
                UsersFocus::Password
            } else {
                UsersFocus::Elsewhere
            };
            let button = match id {
                ID_ADD_USER => Some(UsersButton::AddUser),
                ID_SET_PW => Some(UsersButton::SetPassword),
                _ => None,
            };
            match crate::view::users_enter(button, focus) {
                UsersEnter::AddUser => {
                    let action = Action::AddUser { name: get_text(c.nu_name), password: get_text(c.nu_pw), confirm: get_text(c.nu_confirm) };
                    for h in [c.nu_pw, c.nu_confirm] {
                        set_text(h, "");
                    }
                    Do::Act(action)
                }
                UsersEnter::SetPassword => {
                    let action = Action::SetPassword {
                        user_id: selected_user(ui).unwrap_or_default(),
                        password: get_text(c.new_pw),
                        confirm: get_text(c.confirm_pw),
                    };
                    set_text(c.new_pw, "");
                    set_text(c.confirm_pw, "");
                    Do::Act(action)
                }
                UsersEnter::Nothing => Do::Nothing,
            }
        }
        ID_IDLE_ON_LABEL | ID_IDLE_ON => {
            if id == ID_IDLE_ON_LABEL {
                // SAFETY: our own checkbox.
                unsafe {
                    let on = SendMessageW(c.idle_on, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
                    SendMessageW(c.idle_on, BM_SETCHECK, if on { BST_UNCHECKED as usize } else { BST_CHECKED as usize }, 0);
                }
            }
            ui.last = None; // repaint the enabled state
            Do::Nothing
        }
        ID_IDLE_SAVE => {
            // SAFETY: our own controls.
            let (on, start, margin) = unsafe {
                (
                    SendMessageW(c.idle_on, BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
                    SendMessageW(c.idle_start_ud, UDM_GETPOS32, 0, 0),
                    SendMessageW(c.idle_margin_ud, UDM_GETPOS32, 0, 0),
                )
            };
            Do::Act(Action::SaveIdle { on, start_minutes: start.max(1) as u64, margin_minutes: margin.max(1) as u64 })
        }
        // Enter (IDOK through IsDialogMessage) and the button submit the form.
        ID_SUBMIT | 1 if ui.showing == SETUP => {
            let action = Action::Setup { user: get_text(c.user), password: get_text(c.pass), media: get_text(c.media) };
            set_text(c.pass, "");
            Do::Act(action)
        }
        _ => Do::Nothing,
    }
}

/// Draws one selector-bar item: its name, dimmed unless selected, with an
/// accent underline under the selected one.
fn draw_nav(t: &Paint, dis: &DRAWITEMSTRUCT) {
    let i = (dis.CtlID as usize).wrapping_sub(ID_NAV);
    let selected = i == t.selected;
    // SAFETY: the DC and rect come from Windows for this item.
    unsafe {
        FillRect(dis.hDC, &dis.rcItem, t.bg_brush);
        SetBkMode(dis.hDC, TRANSPARENT as i32);
        SetTextColor(dis.hDC, if selected { t.fg } else { t.dim });
        let mut text: Vec<u16> = NAV.get(i).copied().unwrap_or("").encode_utf16().collect();
        let mut r = dis.rcItem;
        DrawTextW(dis.hDC, text.as_mut_ptr(), text.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        if selected {
            let s = |v: i32| v * t.dpi as i32 / 96;
            let bar = RECT { left: r.left + s(24), right: r.right - s(24), top: r.bottom - s(4), bottom: r.bottom - s(1) };
            let brush = CreateSolidBrush(t.accent);
            FillRect(dis.hDC, &bar, brush);
            DeleteObject(brush as _);
        }
        if dis.itemState & ODS_FOCUS != 0 {
            DrawFocusRect(dis.hDC, &dis.rcItem);
        }
    }
}

/// Whether the message is Windows saying its colour settings changed.
fn is_theme_change(lp: LPARAM) -> bool {
    if lp == 0 {
        return false;
    }
    // SAFETY: WM_SETTINGCHANGE's lParam is a NUL-terminated wide string or null.
    let s = unsafe {
        let p = lp as *const u16;
        let mut n = 0;
        while n < 64 && *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    };
    s == "ImmersiveColorSet"
}

const PAGE_CLASS: &str = "RemoraPage";

/// Fills a window's background with the theme's colour.
unsafe fn erase(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: called from a window procedure with its own arguments.
    unsafe {
        match paint_state() {
            Some(t) => {
                let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetClientRect(hwnd, &mut r);
                FillRect(wp as HDC, &r, t.bg_brush);
                1
            }
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: called by Windows on the UI thread with valid arguments.
    unsafe {
        match msg {
            WM_COMMAND | WM_DRAWITEM | WM_NOTIFY | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
                SendMessageW(GetParent(hwnd), msg, wp, lp)
            }
            WM_ERASEBKGND => erase(hwnd, msg, wp, lp),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: called by Windows on the UI thread with valid arguments.
    unsafe {
        match msg {
            WM_TIMER => {
                let raise = UI.with(|cell| {
                    let Ok(mut b) = cell.try_borrow_mut() else { return false };
                    let Some(ui) = b.as_mut() else { return false };
                    paint(ui);
                    ui.ticks = ui.ticks.wrapping_add(1);
                    if ui.copied > 0 {
                        ui.copied -= 1;
                        if ui.copied == 0 {
                            set_text(ui.c.copy, "Copy");
                        }
                    }
                    let scanning = ui.last.as_ref().is_some_and(|v| v.scanning);
                    if ui.showing == LIBRARY && scanning && ui.ticks % 3 == 0 {
                        ui.ctrl.refresh_library();
                    }
                    ui.ctrl.take_raise()
                });
                if raise {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
                0
            }
            WM_SETTINGCHANGE if is_theme_change(lp) => {
                UI.with(|cell| {
                    if let Ok(mut b) = cell.try_borrow_mut()
                        && let Some(ui) = b.as_mut()
                    {
                        ui.theme = Theme::current();
                        apply_theme(ui);
                    }
                });
                0
            }
            WM_COMMAND => {
                let id = wp & 0xffff;
                let todo = UI.with(|cell| match cell.try_borrow_mut() {
                    Ok(mut b) => b.as_mut().map(|ui| {
                        let d = on_command(ui, id);
                        for &h in &ui.c.nav {
                            InvalidateRect(h, null(), 1);
                        }
                        (d, ui.ctrl.clone())
                    }),
                    Err(_) => None,
                });
                match todo {
                    Some((Do::Act(a), ctrl)) => ctrl.act(a),
                    // The dialogs below run their own message loops, so they are
                    // shown here, outside the borrow of the UI.
                    Some((Do::Confirm(text, action), ctrl)) => {
                        let (text, title) = (w(&text), w(decide::APP_NAME));
                        if MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES {
                            ctrl.act(action);
                        }
                    }
                    Some((Do::AddFolder(library), ctrl)) => {
                        if let Some(folder) = pick_folder(hwnd, &format!("Add a folder to {library}")) {
                            ctrl.act(Action::AddFolder { library, folder });
                        }
                    }
                    Some((Do::Browse, _)) => {
                        if let Some(folder) = pick_folder(hwnd, "Choose the new library's folder") {
                            UI.with(|cell| {
                                if let Ok(b) = cell.try_borrow()
                                    && let Some(ui) = b.as_ref()
                                {
                                    set_text(ui.c.lib_folder, &folder);
                                }
                            });
                        }
                    }
                    Some((Do::Close, _)) => {
                        SendMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    _ => {}
                }
                0
            }
            WM_DRAWITEM => {
                let dis = &*(lp as *const DRAWITEMSTRUCT);
                if let Some(t) = paint_state() {
                    draw_nav(&t, dis);
                }
                1
            }
            WM_ERASEBKGND => erase(hwnd, msg, wp, lp),
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
                let hdc = wp as HDC;
                {
                    if let Some(t) = paint_state() {
                        let h = lp as HWND;
                        let is_edit = msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX || h == t.tv;
                        let text = if h == t.message && t.error { rgb(208, 69, 59) } else { t.fg };
                        // Opaque, in the theme's colour: a transparent label never
                        // clears its old text, which showed as overlapping words.
                        SetTextColor(hdc, text);
                        SetBkColor(hdc, if is_edit { t.edit_bg } else { t.bg });
                        return (if is_edit { t.edit_brush } else { t.bg_brush }) as LRESULT;
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_CLOSE => {
                // Asked outside the borrow: the message box runs its own loop.
                let ctrl = UI.with(|cell| cell.try_borrow().ok().and_then(|b| b.as_ref().map(|ui| ui.ctrl.clone())));
                if ctrl.is_some_and(|c| c.quit_needs_confirm()) {
                    let (text, title) = (
                        w("Something is playing. Quitting stops the server, and the TV stops playing.\n\nQuit anyway?"),
                        w(decide::APP_NAME),
                    );
                    if MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES {
                        return 0;
                    }
                }
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

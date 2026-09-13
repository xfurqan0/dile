//! The settings window: one of it, opened from the tray, hidden rather than destroyed.
//!
//! **Created at run time, not declared in `tauri.conf.json`.** The panel is declared there
//! because it exists from the first frame and is only ever hidden; this window exists because
//! somebody asked for it, and a tray application that builds a webview at start-up for a
//! window most sessions never open has paid for it anyway. Its capability
//! (`capabilities/settings.json`) matches on the label, which is the same whether the window
//! was declared or built.
//!
//! **Closing hides it.** The window keeps its scroll position, its selected group and its
//! webview, so the second visit is instant; the alternative — destroy and rebuild — is a
//! white flash and a form that forgets where the user was. The application does not exit when
//! it closes, because Dile has no main window and never will.
//!
//! **720 × 520 and not resizable**, which is the mockup the maintainer approved on
//! 2026-09-13: a left rail of 168 px and one pane. Not resizable because every group is laid
//! out against that width, and a settings window is not a document.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::ui::Ui;

/// The window label, which is also what `capabilities/settings.json` matches on.
pub const WINDOW: &str = "settings";

/// The page the window loads, relative to `frontendDist`.
const PAGE: &str = "settings/index.html";

/// The width of the window, in logical pixels.
const WIDTH: f64 = 720.0;

/// The height of the window, in logical pixels.
const HEIGHT: f64 = 520.0;

/// Show the settings window, building it the first time.
///
/// Safe to call from any thread and from the tray's menu handler. Every failure is logged
/// rather than propagated: a settings window that will not open is not a reason to take a
/// running dictation application down.
pub fn open(app: &AppHandle) {
    open_at(app, None);
}

/// Show the settings window with one group already selected.
///
/// `group` is one of the `data-section` names the markup carries. It reaches the page as a
/// global the script reads once at start-up, because a fragment in the asset URL is a path
/// component as far as the webview protocol is concerned.
///
/// **Only the debug switch passes anything but `None` today** (`DILE_OPEN_SETTINGS=engine`),
/// and it exists because no script can click a tray menu, let alone the fourth item of a
/// rail inside a webview — so "does the dictionary table still lay out" is otherwise a
/// question nobody can answer from a terminal.
pub fn open_at(app: &AppHandle, group: Option<&str>) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        // Already built, and probably hidden. `set_focus` on a window that is behind another
        // application is what makes a second click on the tray menu feel like it did
        // something.
        let _ = window.unminimize();
        if let Err(error) = window.show() {
            log::warn!("the settings window could not be shown: {error}");
        }
        if let Err(error) = window.set_focus() {
            log::warn!("the settings window could not be focused: {error}");
        }
        return;
    }

    let title = match app.try_state::<Ui>() {
        Some(ui) => ui.strings().text("settings.title"),
        None => String::new(),
    };

    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App(PAGE.into()))
        .title(title)
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(true)
        .decorations(true)
        .center()
        .visible(true);

    if let Some(group) = group.filter(|group| group.chars().all(|c| c.is_ascii_alphabetic())) {
        // Alphabetic only, and then quoted: this string comes from the environment in a debug
        // build, and a value that could carry a quote would be a script injected into a
        // window whose whole point is that nothing can inject one.
        builder = builder.initialization_script(format!("window.__DILE_GROUP__=\"{group}\";"));
    }

    let built = builder.build();

    match built {
        Ok(window) => {
            let hidden = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    // The window is kept, so the next visit is instant and the application
                    // does not exit with it.
                    api.prevent_close();
                    if let Err(error) = hidden.hide() {
                        log::warn!("the settings window could not be hidden: {error}");
                    }
                }
            });
            log::info!("the settings window is open");
        }
        Err(error) => log::error!("the settings window could not be built: {error}"),
    }
}

/// Hide the settings window, if it is there. What Esc does.
pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW)
        && let Err(error) = window.hide()
    {
        log::warn!("the settings window could not be hidden: {error}");
    }
}

/// Put the window's title back in a new language.
pub fn retitle(app: &AppHandle, title: &str) {
    if let Some(window) = app.get_webview_window(WINDOW)
        && let Err(error) = window.set_title(title)
    {
        log::warn!("the settings window title could not be changed: {error}");
    }
}

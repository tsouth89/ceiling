//! Experimental native Windows taskbar host.
//!
//! Unlike the Tauri FloatBar, this surface is a real child of Explorer's
//! `Shell_TrayWnd`.

use crate::floatbar::taskbar::{TaskbarLandmarks, TaskbarLayout};

const WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChildPlacement {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderReadout {
    provider_id: String,
    percent: Option<u8>,
    window_label: String,
    reset: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WidgetModel {
    providers: Vec<ProviderReadout>,
    dark_text: bool,
}

pub fn native_mode_enabled(settings: &codexbar::settings::Settings) -> bool {
    settings.float_bar_enabled && settings.float_bar_style == "taskbar"
}

fn child_placement(
    layout: &TaskbarLayout,
    landmarks: TaskbarLandmarks,
    provider_count: usize,
) -> Option<ChildPlacement> {
    if layout.bounds.width() < layout.bounds.height() || provider_count == 0 {
        return None;
    }

    let start = landmarks.start?;
    let bounds = layout.bounds;
    let overlaps_taskbar_band = |rect: crate::floatbar::placement::Rect| {
        rect.left >= bounds.left
            && rect.right <= bounds.right
            && rect.top < bounds.bottom
            && rect.bottom > bounds.top
    };
    if !overlaps_taskbar_band(start) {
        return None;
    }

    let lane_left = if let Some(widgets) = landmarks.widgets {
        if !overlaps_taskbar_band(widgets) || widgets.right >= start.left {
            return None;
        }
        widgets.right.saturating_add(8)
    } else {
        bounds.left.saturating_add(8)
    };
    let lane_right = start.left.saturating_sub(8);
    let provider_count = i32::try_from(provider_count).ok()?;
    let desired_width = provider_count.saturating_mul(92);
    let minimum_width = provider_count.saturating_mul(72);

    // UI Automation can expose Search, Task View, or pinned-app buttons in
    // the apparent Widgets-to-Start lane. Never cover one: use only a fully
    // empty sub-gap and hide the proof if no verified gap can fit.
    let mut obstacles = layout
        .obstacles
        .iter()
        .copied()
        .filter(|rect| {
            rect.top < bounds.bottom
                && rect.bottom > bounds.top
                && rect.right > lane_left
                && rect.left < lane_right
        })
        .collect::<Vec<_>>();
    obstacles.sort_by_key(|rect| (rect.left, rect.right));

    let mut gap_left = lane_left;
    let mut gaps = Vec::new();
    for obstacle in obstacles {
        let obstacle_left = obstacle.left.max(lane_left);
        if obstacle_left.saturating_sub(gap_left) >= minimum_width {
            gaps.push((gap_left, obstacle_left));
        }
        gap_left = gap_left.max(obstacle.right.saturating_add(8));
    }
    if lane_right.saturating_sub(gap_left) >= minimum_width {
        gaps.push((gap_left, lane_right));
    }
    let (gap_left, gap_right) = gaps
        .into_iter()
        .max_by_key(|(left, right)| right.saturating_sub(*left))?;
    let available_width = gap_right.saturating_sub(gap_left);

    let taskbar_height = layout.bounds.height();
    let width = desired_width.min(available_width);

    Some(ChildPlacement {
        x: gap_left
            .saturating_add(available_width.saturating_sub(width) / 2)
            .saturating_sub(layout.bounds.left),
        y: 0,
        width,
        height: taskbar_height,
    })
}

pub fn install(app: &tauri::AppHandle) {
    #[cfg(windows)]
    windows_host::install(app);
    #[cfg(not(windows))]
    let _ = app;
}

pub fn apply_state(app: &tauri::AppHandle, settings: &codexbar::settings::Settings) {
    #[cfg(windows)]
    windows_host::apply_state(app, settings);
    #[cfg(not(windows))]
    let _ = (app, settings);
}

/// Return a sampled RGB color from Explorer's current taskbar material. The
/// native flyout uses this as its base tint so custom Windows accent colors do
/// not leave Ceiling looking like a separate, bolted-on surface.
#[tauri::command]
pub fn get_taskbar_surface_color() -> Option<String> {
    #[cfg(windows)]
    return windows_host::taskbar_surface_color();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod windows_host {
    use super::*;
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    use tauri::Manager;

    const CLASS_NAME: &str = "CeilingNativeTaskbarWidget";
    const WINDOW_TITLE: &str = "Ceiling taskbar widget proof";

    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    const LWA_COLORKEY: u32 = 0x0000_0001;
    const LWA_ALPHA: u32 = 0x0000_0002;
    const SW_HIDE: i32 = 0;
    const SW_SHOWNA: i32 = 8;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_NOOWNERZORDER: u32 = 0x0200;

    const WM_DESTROY: u32 = 0x0002;
    const WM_PAINT: u32 = 0x000F;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_SETCURSOR: u32 = 0x0020;
    const WM_MOUSEACTIVATE: u32 = 0x0021;
    const WM_LBUTTONUP: u32 = 0x0202;
    const MA_NOACTIVATE: isize = 3;
    const IDC_HAND: usize = 32649;
    const TRANSPARENT: i32 = 1;
    const PS_SOLID: i32 = 0;
    const FONT_QUALITY_ANTIALIASED: u32 = 4;
    // A deliberately uncommon key color. Pixels left in this color are
    // transparent, allowing Explorer's own taskbar material to show through.
    const TRANSPARENT_KEY: u32 = rgb(1, 2, 3);

    #[derive(Debug, Default)]
    struct HostState {
        hwnd: isize,
        taskbar: isize,
        model: WidgetModel,
    }

    struct PreparedWidget {
        taskbar: isize,
        placement: ChildPlacement,
        model: WidgetModel,
    }

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static HOST: OnceLock<Mutex<HostState>> = OnceLock::new();
    static CLASS_REGISTERED: OnceLock<bool> = OnceLock::new();
    static RECOVERY_PENDING: AtomicBool = AtomicBool::new(false);

    pub(super) fn install(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        schedule_recovery(app);

        let refresh_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::commands::do_refresh_providers_if_stale(&refresh_app).await;
            schedule_recovery(&refresh_app);
        });

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(WATCHDOG_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                schedule_recovery(&app);
            }
        });
    }

    pub(super) fn apply_state(app: &tauri::AppHandle, settings: &codexbar::settings::Settings) {
        if native_mode_enabled(settings) {
            schedule_recovery(app);
        } else {
            hide_existing();
        }
    }

    pub(super) fn taskbar_surface_color() -> Option<String> {
        let taskbar = unsafe { find_primary_taskbar()? };
        let mut rect = WinRect::default();
        if unsafe { GetWindowRect(taskbar, (&mut rect as *mut WinRect).cast()) } == 0 {
            return None;
        }
        let dc = unsafe { GetDC(0) };
        if dc == 0 {
            return None;
        }

        // The upper edge is normally free of buttons, hover states, and text.
        // Sample several points and take the median per channel to reject an
        // occasional icon/accent pixel without needing screen capture APIs.
        let width = rect.right.saturating_sub(rect.left).max(1);
        let y = rect.top.saturating_add(3);
        let mut reds = Vec::new();
        let mut greens = Vec::new();
        let mut blues = Vec::new();
        for fraction in [1, 2, 3, 4, 5] {
            let x = rect.left.saturating_add(width.saturating_mul(fraction) / 6);
            let color = unsafe { GetPixel(dc, x, y) };
            if color == u32::MAX {
                continue;
            }
            reds.push((color & 0xff) as u8);
            greens.push(((color >> 8) & 0xff) as u8);
            blues.push(((color >> 16) & 0xff) as u8);
        }
        unsafe { ReleaseDC(0, dc) };
        if reds.is_empty() {
            return None;
        }
        reds.sort_unstable();
        greens.sort_unstable();
        blues.sort_unstable();
        let middle = reds.len() / 2;
        Some(format!(
            "#{:02x}{:02x}{:02x}",
            reds[middle], greens[middle], blues[middle]
        ))
    }

    fn schedule_recovery(app: &tauri::AppHandle) {
        if RECOVERY_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let prepared = tauri::async_runtime::spawn_blocking(prepare_widget).await;
            let dispatched = app.run_on_main_thread(move || {
                match prepared {
                    Ok(Ok(prepared)) => {
                        if let Err(error) = apply_prepared(prepared) {
                            tracing::warn!(%error, "Native taskbar widget proof update failed");
                        }
                    }
                    Ok(Err(error)) => {
                        hide_existing();
                        tracing::debug!(%error, "Native taskbar widget proof recovery deferred");
                    }
                    Err(error) => {
                        hide_existing();
                        tracing::warn!(%error, "Native taskbar discovery worker failed");
                    }
                }
                RECOVERY_PENDING.store(false, Ordering::Release);
            });
            if dispatched.is_err() {
                RECOVERY_PENDING.store(false, Ordering::Release);
            }
        });
    }

    fn prepare_widget() -> Result<PreparedWidget, String> {
        let settings = codexbar::settings::Settings::load();
        if !native_mode_enabled(&settings) {
            return Err("Native taskbar mode is disabled".to_string());
        }
        let model = widget_model()?;
        if model.providers.is_empty() {
            return Err("No enabled providers are available for the taskbar widget".to_string());
        }
        let taskbar = unsafe { find_primary_taskbar() }
            .ok_or_else(|| "Explorer primary taskbar is unavailable".to_string())?;
        let layouts = crate::floatbar::taskbar::discover_all();
        let layout = crate::floatbar::taskbar::primary_layout(&layouts)
            .ok_or_else(|| "No usable primary taskbar layout was discovered".to_string())?;
        let landmarks = crate::floatbar::taskbar::primary_landmarks();
        let Some(placement) = child_placement(layout, landmarks, model.providers.len()) else {
            return Err(
                "The Widgets-to-Start taskbar lane cannot fit the native widget".to_string(),
            );
        };

        Ok(PreparedWidget {
            taskbar,
            placement,
            model,
        })
    }

    fn apply_prepared(prepared: PreparedWidget) -> Result<(), String> {
        let mut state = HOST
            .get_or_init(|| Mutex::new(HostState::default()))
            .lock()
            .map_err(|_| "Native taskbar widget state is poisoned".to_string())?;

        let window_alive = state.hwnd != 0 && unsafe { IsWindow(state.hwnd) } != 0;
        let correctly_parented = window_alive
            && state.taskbar == prepared.taskbar
            && unsafe { GetParent(state.hwnd) } == prepared.taskbar;
        if !correctly_parented {
            // Never reparent a stale foreign child after Explorer restarts.
            // Destroy it on its owner thread and create a fresh popup host for
            // the new Shell_TrayWnd instead.
            if window_alive {
                unsafe { DestroyWindow(state.hwnd) };
            }
            state.hwnd = unsafe { create_widget(prepared.taskbar)? };
            state.taskbar = prepared.taskbar;
            tracing::info!("Created native Ceiling taskbar widget proof");
        }

        let model_changed = state.model != prepared.model;
        state.model = prepared.model;
        unsafe {
            SetWindowRgn(state.hwnd, 0, 1);
            SetWindowPos(
                state.hwnd,
                0,
                prepared.placement.x,
                prepared.placement.y,
                prepared.placement.width,
                prepared.placement.height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
            ShowWindow(state.hwnd, SW_SHOWNA);
            if model_changed {
                InvalidateRect(state.hwnd, std::ptr::null(), 0);
            }
        }
        Ok(())
    }

    fn widget_model() -> Result<WidgetModel, String> {
        let app = APP
            .get()
            .ok_or_else(|| "Native taskbar widget app handle is unavailable".to_string())?;
        let settings = codexbar::settings::Settings::load();
        let preferred_ids = if settings.float_bar_provider_ids.is_empty() {
            settings.provider_display_order_names()
        } else {
            settings.float_bar_provider_ids.clone()
        };
        let state = app.state::<Mutex<crate::state::AppState>>();
        let guard = state
            .lock()
            .map_err(|_| "Ceiling provider state is poisoned".to_string())?;

        let providers = preferred_ids
            .into_iter()
            .filter(|provider_id| settings.enabled_providers.contains(provider_id))
            .map(|provider_id| {
                let snapshot = guard
                    .provider_cache
                    .iter()
                    .find(|snapshot| snapshot.provider_id == provider_id);
                let percent =
                    snapshot
                        .filter(|snapshot| snapshot.error.is_none())
                        .map(|snapshot| {
                            let value = if settings.show_as_used {
                                snapshot.primary.used_percent
                            } else {
                                snapshot.primary.remaining_percent
                            };
                            value.clamp(0.0, 100.0).round() as u8
                        });
                ProviderReadout {
                    provider_id,
                    percent,
                    window_label: compact_window_label(
                        snapshot.and_then(|snapshot| snapshot.primary_label.as_deref()),
                        snapshot.and_then(|snapshot| snapshot.primary.window_minutes),
                    ),
                    reset: settings
                        .float_bar_show_reset_inline
                        .then(|| {
                            snapshot.and_then(|snapshot| {
                                crate::tray_bridge::tooltip_short_reset(
                                    snapshot.primary.resets_at.as_deref(),
                                    snapshot.primary.reset_description.as_deref(),
                                )
                            })
                        })
                        .flatten(),
                }
            })
            .take(3)
            .collect();

        let dark_text = match codexbar::settings::resolved_float_bar_contrast(&settings).as_str() {
            "dark-text" => true,
            "light-text" => false,
            _ => system_uses_light_theme(),
        };

        Ok(WidgetModel {
            providers,
            dark_text,
        })
    }

    fn system_uses_light_theme() -> bool {
        const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as isize;
        const RRF_RT_REG_DWORD: u32 = 0x0000_0018;
        let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let name = wide("SystemUsesLightTheme");
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            ) == 0
                && value != 0
        }
    }

    fn compact_window_label(label: Option<&str>, window_minutes: Option<u32>) -> String {
        let label = label.map(str::trim).filter(|label| !label.is_empty());
        if let Some(label) = label {
            let normalized = label.to_ascii_lowercase();
            if normalized.contains("5-hour") || normalized.contains("5 hour") {
                return "5h".to_string();
            }
            if normalized.contains("weekly") || normalized == "week" {
                return "Weekly".to_string();
            }
            if normalized.contains("monthly") || normalized == "month" {
                return "Monthly".to_string();
            }
            if normalized.contains("session") && window_minutes == Some(300) {
                return "5h".to_string();
            }
            return label.chars().take(9).collect();
        }

        match window_minutes {
            Some(minutes) if minutes <= 360 => format!("{}h", (minutes / 60).max(1)),
            Some(minutes) if minutes <= 10_080 => "Weekly".to_string(),
            Some(minutes) if minutes >= 40_320 => "Monthly".to_string(),
            _ => "Usage".to_string(),
        }
    }

    fn hide_existing() {
        let Some(host) = HOST.get() else {
            return;
        };
        let Ok(state) = host.try_lock() else {
            return;
        };
        if state.hwnd != 0 && unsafe { IsWindow(state.hwnd) } != 0 {
            unsafe { ShowWindow(state.hwnd, SW_HIDE) };
        }
    }

    unsafe fn find_primary_taskbar() -> Option<isize> {
        let class = wide("Shell_TrayWnd");
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        (hwnd != 0).then_some(hwnd)
    }

    unsafe fn create_widget(taskbar: isize) -> Result<isize, String> {
        if !*CLASS_REGISTERED.get_or_init(|| unsafe { register_class() }) {
            return Err("Could not register the native widget window class".to_string());
        }
        let class = wide(CLASS_NAME);
        let title = wide(WINDOW_TITLE);
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                taskbar,
                0,
                instance,
                std::ptr::null(),
            )
        };
        if hwnd == 0 {
            return Err("CreateWindowExW failed for the taskbar widget".to_string());
        }
        let previous = unsafe { SetParent(hwnd, taskbar) };
        if previous == 0 && unsafe { GetParent(hwnd) } != taskbar {
            unsafe { DestroyWindow(hwnd) };
            return Err("Could not attach native widget to the taskbar".to_string());
        }
        if unsafe { GetParent(hwnd) } != taskbar {
            unsafe { DestroyWindow(hwnd) };
            return Err("Could not attach native widget to the taskbar".to_string());
        }
        if unsafe {
            SetLayeredWindowAttributes(hwnd, TRANSPARENT_KEY, 255, LWA_COLORKEY | LWA_ALPHA)
        } == 0
        {
            unsafe { DestroyWindow(hwnd) };
            return Err("Could not enable native widget composition".to_string());
        }
        Ok(hwnd)
    }

    unsafe fn register_class() -> bool {
        let class = wide(CLASS_NAME);
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let wc = WndClassExW {
            size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            window_proc: Some(widget_window_proc),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: 0,
            cursor: unsafe { LoadCursorW(0, IDC_HAND as *const u16) },
            background: 0,
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            small_icon: 0,
        };
        unsafe { RegisterClassExW(&wc) != 0 }
    }

    unsafe extern "system" fn widget_window_proc(
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match message {
            WM_PAINT => {
                unsafe { paint_widget(hwnd) };
                0
            }
            WM_ERASEBKGND => 1,
            WM_MOUSEACTIVATE => MA_NOACTIVATE,
            WM_SETCURSOR => {
                unsafe { SetCursor(LoadCursorW(0, IDC_HAND as *const u16)) };
                1
            }
            WM_LBUTTONUP => {
                toggle_flyout(hwnd);
                0
            }
            WM_DESTROY => 0,
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn toggle_flyout(hwnd: isize) {
        let Some(app) = APP.get().cloned() else {
            return;
        };

        // Treat the widget rectangle as the tray anchor so the existing
        // compact flyout opens visually connected to this taskbar surface.
        // Never wait for AppState from Explorer's mouse-message path.
        let mut rect = WinRect::default();
        if unsafe { GetWindowRect(hwnd, (&mut rect as *mut WinRect).cast()) } != 0
            && let Some(state) = app.try_state::<Mutex<crate::state::AppState>>()
            && let Ok(mut state) = state.try_lock()
        {
            state.tray_anchor = Some(crate::state::TrayAnchor {
                x: rect.left,
                y: rect.top,
                width: rect.right.saturating_sub(rect.left).max(1) as u32,
                height: rect.bottom.saturating_sub(rect.top).max(1) as u32,
            });
        }

        tauri::async_runtime::spawn(async move {
            let flyout = app.get_webview_window(crate::shell::flyout_window::FLYOUT_LABEL);
            let visible = flyout
                .as_ref()
                .and_then(|window| window.is_visible().ok())
                .unwrap_or(false);
            let result = if visible {
                crate::shell::flyout_window::hide(&app)
            } else {
                crate::shell::flyout_window::open_or_focus(&app, None)
            };
            if let Err(error) = result {
                tracing::warn!(%error, "Could not toggle native taskbar widget flyout");
            }
        });
    }

    unsafe fn paint_widget(hwnd: isize) {
        let mut paint = PaintStruct::default();
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        if hdc == 0 {
            return;
        }

        let model = HOST
            .get()
            .and_then(|host| host.try_lock().ok().map(|state| state.model.clone()))
            .unwrap_or_default();
        let mut rect = WinRect::default();
        unsafe { GetClientRect(hwnd, &mut rect) };
        let background = unsafe { CreateSolidBrush(TRANSPARENT_KEY) };
        unsafe {
            FillRect(hdc, &rect, background);
            DeleteObject(background);
            SetBkMode(hdc, TRANSPARENT);
        }

        let face = wide("Segoe UI Variable Text");
        let primary_font = unsafe {
            CreateFontW(
                -15,
                0,
                0,
                0,
                500,
                0,
                0,
                0,
                1,
                0,
                0,
                FONT_QUALITY_ANTIALIASED,
                0,
                face.as_ptr(),
            )
        };
        let detail_font = unsafe {
            CreateFontW(
                -12,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                FONT_QUALITY_ANTIALIASED,
                0,
                face.as_ptr(),
            )
        };
        let old_font = unsafe { SelectObject(hdc, primary_font) };
        let count = i32::try_from(model.providers.len()).unwrap_or(1).max(1);
        let item_width = (rect.right - rect.left) / count;
        let middle = (rect.bottom - rect.top) / 2;
        let text_color = if model.dark_text {
            rgb(24, 24, 24)
        } else {
            rgb(255, 255, 255)
        };

        for (index, provider) in model.providers.iter().enumerate() {
            let item_left = i32::try_from(index).unwrap_or(0) * item_width;
            let color = provider_color(&provider.provider_id);
            unsafe {
                draw_provider_icon(
                    hdc,
                    &provider.provider_id,
                    item_left + 17,
                    middle - 7,
                    color,
                )
            };
            let label = provider
                .percent
                .map(|percent| format!("{percent}%"))
                .unwrap_or_else(|| "—".to_string());
            let label = wide_without_nul(&label);
            unsafe {
                SetTextColor(hdc, text_color);
                TextOutW(
                    hdc,
                    item_left + 30,
                    middle - 16,
                    label.as_ptr(),
                    label.len() as i32,
                );
            }

            let detail = match provider.reset.as_deref() {
                Some(reset) => format!("{} · {reset}", provider.window_label),
                None => provider.window_label.clone(),
            };
            let detail: String = detail.chars().take(15).collect();
            let detail = wide_without_nul(&detail);
            unsafe {
                SelectObject(hdc, detail_font);
                SetTextColor(hdc, text_color);
                TextOutW(
                    hdc,
                    item_left + 8,
                    middle + 1,
                    detail.as_ptr(),
                    detail.len() as i32,
                );
                SelectObject(hdc, primary_font);
            }

            if index + 1 < model.providers.len() {
                let separator = unsafe { CreatePen(PS_SOLID, 1, rgb(118, 127, 140)) };
                let old_pen = unsafe { SelectObject(hdc, separator) };
                unsafe {
                    MoveToEx(
                        hdc,
                        item_left + item_width - 1,
                        middle - 13,
                        std::ptr::null_mut(),
                    );
                    LineTo(hdc, item_left + item_width - 1, middle + 13);
                    SelectObject(hdc, old_pen);
                    DeleteObject(separator);
                }
            }
        }

        unsafe {
            SelectObject(hdc, old_font);
            DeleteObject(primary_font);
            DeleteObject(detail_font);
            EndPaint(hwnd, &paint);
        }
    }

    unsafe fn draw_provider_icon(hdc: isize, provider_id: &str, x: i32, y: i32, color: u32) {
        const CODEX: [u16; 16] = [
            0x0000, 0x0000, 0x03e0, 0x1f10, 0x10d8, 0x2f54, 0x39d4, 0x2654, 0x2a64, 0x2bdc, 0x2af4,
            0x1b08, 0x0cf8, 0x07c0, 0x0000, 0x0000,
        ];
        const CLAUDE: [u16; 16] = [
            0x0000, 0x0000, 0x0320, 0x1b60, 0x0d48, 0x0ff8, 0x07e0, 0x3fc4, 0x1ff8, 0x37e0, 0x0ff8,
            0x1f68, 0x04a0, 0x0080, 0x0000, 0x0000,
        ];
        const CURSOR: [u16; 16] = [
            0x0000, 0x0000, 0x03c0, 0x0ff0, 0x1ff8, 0x200c, 0x303c, 0x307c, 0x38fc, 0x38fc, 0x3cfc,
            0x1ef8, 0x0ef0, 0x03c0, 0x0000, 0x0000,
        ];

        let mask = match provider_id {
            "codex" => Some(&CODEX),
            "claude" => Some(&CLAUDE),
            "cursor" => Some(&CURSOR),
            _ => None,
        };
        if let Some(mask) = mask {
            unsafe { draw_icon_mask(hdc, mask, x - 8, y - 8, color) };
            return;
        }

        let pen = unsafe { CreatePen(PS_SOLID, 2, color) };
        let old_pen = unsafe { SelectObject(hdc, pen) };
        match provider_id {
            "claude" => {
                for (dx, dy) in [(0, 7), (5, 5), (7, 0), (5, -5)] {
                    unsafe {
                        MoveToEx(hdc, x - dx, y - dy, std::ptr::null_mut());
                        LineTo(hdc, x + dx, y + dy);
                    }
                }
            }
            "cursor" => {
                let brush = unsafe { CreateSolidBrush(color) };
                let old_brush = unsafe { SelectObject(hdc, brush) };
                let points = [
                    WinPoint { x, y: y - 8 },
                    WinPoint { x: x + 8, y },
                    WinPoint { x, y: y + 8 },
                    WinPoint { x: x - 8, y },
                ];
                unsafe {
                    Polygon(hdc, points.as_ptr(), points.len() as i32);
                    SelectObject(hdc, old_brush);
                    DeleteObject(brush);
                }
            }
            _ => unsafe {
                Ellipse(hdc, x - 8, y - 8, x + 8, y + 8);
                Ellipse(hdc, x - 4, y - 4, x + 4, y + 4);
            },
        }
        unsafe {
            SelectObject(hdc, old_pen);
            DeleteObject(pen);
        }
    }

    unsafe fn draw_icon_mask(hdc: isize, rows: &[u16; 16], x: i32, y: i32, color: u32) {
        for (row_index, row) in rows.iter().copied().enumerate() {
            for column in 0..16 {
                if row & (1 << column) != 0 {
                    unsafe { SetPixelV(hdc, x + column, y + row_index as i32, color) };
                }
            }
        }
    }

    fn provider_color(provider_id: &str) -> u32 {
        match provider_id {
            "claude" => rgb(216, 116, 75),
            "cursor" => rgb(15, 201, 181),
            "codex" => rgb(64, 196, 222),
            _ => rgb(204, 211, 220),
        }
    }

    const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
        red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_without_nul(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    #[repr(C)]
    struct WndClassExW {
        size: u32,
        style: u32,
        window_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
        class_extra: i32,
        window_extra: i32,
        instance: isize,
        icon: isize,
        cursor: isize,
        background: isize,
        menu_name: *const u16,
        class_name: *const u16,
        small_icon: isize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WinPoint {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct PaintStruct {
        hdc: isize,
        erase: i32,
        paint: WinRect,
        restore: i32,
        incremental_update: i32,
        reserved: [u8; 32],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassExW(class: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            extended_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const std::ffi::c_void,
        ) -> isize;
        fn DefWindowProcW(hwnd: isize, message: u32, wparam: usize, lparam: isize) -> isize;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
        fn GetParent(hwnd: isize) -> isize;
        fn SetParent(child: isize, new_parent: isize) -> isize;
        fn SetLayeredWindowAttributes(hwnd: isize, color_key: u32, alpha: u8, flags: u32) -> i32;
        fn DestroyWindow(hwnd: isize) -> i32;
        fn IsWindow(hwnd: isize) -> i32;
        fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn ShowWindow(hwnd: isize, command: i32) -> i32;
        fn InvalidateRect(hwnd: isize, rect: *const WinRect, erase: i32) -> i32;
        fn LoadCursorW(instance: isize, cursor_name: *const u16) -> isize;
        fn SetCursor(cursor: isize) -> isize;
        fn BeginPaint(hwnd: isize, paint: *mut PaintStruct) -> isize;
        fn EndPaint(hwnd: isize, paint: *const PaintStruct) -> i32;
        fn GetClientRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut std::ffi::c_void) -> i32;
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
        fn FillRect(hdc: isize, rect: *const WinRect, brush: isize) -> i32;
        fn SetWindowRgn(hwnd: isize, region: isize, redraw: i32) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            value_type: *mut u32,
            data: *mut std::ffi::c_void,
            data_size: *mut u32,
        ) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetPixel(hdc: isize, x: i32, y: i32) -> u32;
        fn CreateSolidBrush(color: u32) -> isize;
        fn CreatePen(style: i32, width: i32, color: u32) -> isize;
        fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike_out: u32,
            char_set: u32,
            output_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch_and_family: u32,
            face: *const u16,
        ) -> isize;
        fn DeleteObject(object: isize) -> i32;
        fn SelectObject(hdc: isize, object: isize) -> isize;
        fn SetBkMode(hdc: isize, mode: i32) -> i32;
        fn SetTextColor(hdc: isize, color: u32) -> u32;
        fn TextOutW(hdc: isize, x: i32, y: i32, text: *const u16, count: i32) -> i32;
        fn MoveToEx(hdc: isize, x: i32, y: i32, previous: *mut WinPoint) -> i32;
        fn LineTo(hdc: isize, x: i32, y: i32) -> i32;
        fn Polygon(hdc: isize, points: *const WinPoint, count: i32) -> i32;
        fn Ellipse(hdc: isize, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        fn SetPixelV(hdc: isize, x: i32, y: i32, color: u32) -> i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floatbar::placement::Rect;

    fn layout(bounds: Rect, obstacles: Vec<Rect>) -> TaskbarLayout {
        TaskbarLayout {
            bounds,
            monitor_bounds: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            obstacles,
            primary: true,
        }
    }

    fn landmarks(widgets: Rect, start: Rect) -> TaskbarLandmarks {
        TaskbarLandmarks {
            widgets: Some(widgets),
            start: Some(start),
        }
    }

    #[test]
    fn native_widget_uses_left_lane_taskbar_client_coordinates() {
        let taskbar = layout(
            Rect {
                left: -1920,
                top: 1032,
                right: 0,
                bottom: 1080,
            },
            vec![
                Rect {
                    left: -1920,
                    top: 1032,
                    right: -1760,
                    bottom: 1080,
                },
                Rect {
                    left: -1200,
                    top: 1032,
                    right: -800,
                    bottom: 1080,
                },
            ],
        );

        let placement = child_placement(
            &taskbar,
            landmarks(
                Rect {
                    left: -1920,
                    top: 1032,
                    right: -1760,
                    bottom: 1080,
                },
                Rect {
                    left: -1100,
                    top: 1032,
                    right: -1052,
                    bottom: 1080,
                },
            ),
            3,
        )
        .expect("the Widgets-to-Start lane should fit");
        assert_eq!(placement.x, 306);
        assert_eq!(placement.y, 0);
        assert_eq!(placement.width, 276);
        assert_eq!(placement.height, 48);
    }

    #[test]
    fn proof_widget_refuses_vertical_taskbars_for_now() {
        let taskbar = layout(
            Rect {
                left: 0,
                top: 0,
                right: 48,
                bottom: 1080,
            },
            vec![],
        );
        assert_eq!(
            child_placement(
                &taskbar,
                landmarks(
                    Rect {
                        left: 0,
                        top: 0,
                        right: 48,
                        bottom: 60,
                    },
                    Rect {
                        left: 0,
                        top: 500,
                        right: 48,
                        bottom: 548,
                    },
                ),
                3,
            ),
            None
        );
    }

    #[test]
    fn proof_widget_hides_when_no_complete_gap_exists() {
        let taskbar = layout(
            Rect {
                left: 0,
                top: 1032,
                right: 500,
                bottom: 1080,
            },
            vec![Rect {
                left: 0,
                top: 1032,
                right: 500,
                bottom: 1080,
            }],
        );
        assert_eq!(
            child_placement(
                &taskbar,
                landmarks(
                    Rect {
                        left: 0,
                        top: 1032,
                        right: 200,
                        bottom: 1080,
                    },
                    Rect {
                        left: 340,
                        top: 1032,
                        right: 388,
                        bottom: 1080,
                    },
                ),
                3,
            ),
            None
        );
    }

    #[test]
    fn native_widget_uses_taskbar_edge_when_windows_widgets_are_disabled() {
        let taskbar = layout(
            Rect {
                left: 0,
                top: 1032,
                right: 1920,
                bottom: 1080,
            },
            vec![],
        );
        let placement = child_placement(
            &taskbar,
            TaskbarLandmarks {
                widgets: None,
                start: Some(Rect {
                    left: 800,
                    top: 1032,
                    right: 848,
                    bottom: 1080,
                }),
            },
            3,
        )
        .expect("the taskbar edge-to-Start lane should fit");
        assert_eq!(placement.x, 262);
        assert_eq!(placement.width, 276);
    }

    #[test]
    fn native_widget_rejects_stale_landmarks_outside_the_taskbar() {
        let taskbar = layout(
            Rect {
                left: 0,
                top: 1032,
                right: 1920,
                bottom: 1080,
            },
            vec![],
        );

        assert_eq!(
            child_placement(
                &taskbar,
                landmarks(
                    Rect {
                        left: -160,
                        top: 1032,
                        right: 0,
                        bottom: 1080,
                    },
                    Rect {
                        left: 800,
                        top: 1032,
                        right: 848,
                        bottom: 1080,
                    },
                ),
                3,
            ),
            None
        );
    }

    #[test]
    fn native_widget_uses_only_a_verified_empty_sub_gap() {
        let taskbar = layout(
            Rect {
                left: 0,
                top: 1032,
                right: 1920,
                bottom: 1080,
            },
            vec![Rect {
                left: 300,
                top: 1032,
                right: 420,
                bottom: 1080,
            }],
        );

        let placement = child_placement(
            &taskbar,
            landmarks(
                Rect {
                    left: 0,
                    top: 1032,
                    right: 160,
                    bottom: 1080,
                },
                Rect {
                    left: 800,
                    top: 1032,
                    right: 848,
                    bottom: 1080,
                },
            ),
            3,
        )
        .expect("the verified gap after the obstacle should fit");

        assert_eq!(placement.x, 472);
        assert_eq!(placement.width, 276);
    }
}

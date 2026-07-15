//! Experimental native Windows taskbar host.
//!
//! Unlike the Tauri FloatBar, this surface is a real child of Explorer's
//! `Shell_TrayWnd`. The proof is deliberately gated behind
//! `CEILING_NATIVE_TASKBAR_WIDGET=1` until its lifecycle and placement have
//! been manually validated on supported Windows configurations.

use crate::floatbar::taskbar::{TaskbarLandmarks, TaskbarLayout};

const ENABLE_ENV: &str = "CEILING_NATIVE_TASKBAR_WIDGET";
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WidgetModel {
    providers: Vec<ProviderReadout>,
}

fn flag_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub fn proof_enabled() -> bool {
    flag_enabled(std::env::var(ENABLE_ENV).ok().as_deref())
}

fn child_placement(
    layout: &TaskbarLayout,
    landmarks: TaskbarLandmarks,
    provider_count: usize,
) -> Option<ChildPlacement> {
    if layout.bounds.width() < layout.bounds.height() || provider_count == 0 {
        return None;
    }

    let widgets = landmarks.widgets?;
    let start = landmarks.start?;
    let bounds = layout.bounds;
    let overlaps_taskbar_band = |rect: crate::floatbar::placement::Rect| {
        rect.left >= bounds.left
            && rect.right <= bounds.right
            && rect.top < bounds.bottom
            && rect.bottom > bounds.top
    };
    if !overlaps_taskbar_band(widgets)
        || !overlaps_taskbar_band(start)
        || widgets.right >= start.left
    {
        return None;
    }

    let lane_left = widgets.right.saturating_add(8);
    let lane_right = start.left.saturating_sub(8);
    let provider_count = i32::try_from(provider_count).ok()?;
    let desired_width = provider_count.saturating_mul(80);
    let minimum_width = provider_count.saturating_mul(62);

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
    let mut selected = None;
    for obstacle in obstacles {
        let obstacle_left = obstacle.left.max(lane_left);
        if obstacle_left.saturating_sub(gap_left) >= minimum_width {
            selected = Some((gap_left, obstacle_left));
            break;
        }
        gap_left = gap_left.max(obstacle.right.saturating_add(8));
    }
    if selected.is_none() && lane_right.saturating_sub(gap_left) >= minimum_width {
        selected = Some((gap_left, lane_right));
    }
    let (gap_left, gap_right) = selected?;
    let available_width = gap_right.saturating_sub(gap_left);

    let taskbar_height = layout.bounds.height();
    let width = desired_width.min(available_width);

    Some(ChildPlacement {
        x: gap_left.saturating_sub(layout.bounds.left),
        y: 0,
        width,
        height: taskbar_height,
    })
}

pub fn install(app: &tauri::AppHandle) {
    if !proof_enabled() {
        return;
    }

    #[cfg(windows)]
    windows_host::install(app);
    #[cfg(not(windows))]
    let _ = app;
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
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
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
    const PROOF_BACKGROUND: u32 = rgb(24, 34, 52);

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
                }
            })
            .take(3)
            .collect();

        Ok(WidgetModel { providers })
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
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                0,
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
        let background = unsafe { CreateSolidBrush(PROOF_BACKGROUND) };
        unsafe {
            FillRect(hdc, &rect, background);
            DeleteObject(background);
            SetBkMode(hdc, TRANSPARENT);
        }

        let face = wide("Segoe UI Variable Text");
        let font = unsafe {
            CreateFontW(
                -16,
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
        let old_font = unsafe { SelectObject(hdc, font) };
        let count = i32::try_from(model.providers.len()).unwrap_or(1).max(1);
        let item_width = (rect.right - rect.left) / count;
        let middle = (rect.bottom - rect.top) / 2;

        for (index, provider) in model.providers.iter().enumerate() {
            let item_left = i32::try_from(index).unwrap_or(0) * item_width;
            let color = provider_color(&provider.provider_id);
            unsafe {
                draw_provider_icon(hdc, &provider.provider_id, item_left + 15, middle, color)
            };
            let label = provider
                .percent
                .map(|percent| format!("{percent}%"))
                .unwrap_or_else(|| "—".to_string());
            let label = wide_without_nul(&label);
            unsafe {
                SetTextColor(hdc, color);
                TextOutW(
                    hdc,
                    item_left + 30,
                    middle - 8,
                    label.as_ptr(),
                    label.len() as i32,
                );
            }

            if index + 1 < model.providers.len() {
                let separator = unsafe { CreatePen(PS_SOLID, 1, rgb(76, 84, 94)) };
                let old_pen = unsafe { SelectObject(hdc, separator) };
                unsafe {
                    MoveToEx(
                        hdc,
                        item_left + item_width - 1,
                        middle - 9,
                        std::ptr::null_mut(),
                    );
                    LineTo(hdc, item_left + item_width - 1, middle + 9);
                    SelectObject(hdc, old_pen);
                    DeleteObject(separator);
                }
            }
        }

        unsafe {
            SelectObject(hdc, old_font);
            DeleteObject(font);
            EndPaint(hwnd, &paint);
        }
    }

    unsafe fn draw_provider_icon(hdc: isize, provider_id: &str, x: i32, y: i32, color: u32) {
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
        fn FillRect(hdc: isize, rect: *const WinRect, brush: isize) -> i32;
        fn SetWindowRgn(hwnd: isize, region: isize, redraw: i32) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
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
    fn flag_parser_is_explicit() {
        assert!(flag_enabled(Some("1")));
        assert!(flag_enabled(Some(" TRUE ")));
        assert!(flag_enabled(Some("on")));
        assert!(!flag_enabled(Some("0")));
        assert!(!flag_enabled(None));
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
        assert_eq!(placement.x, 168);
        assert_eq!(placement.y, 0);
        assert_eq!(placement.width, 240);
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
    fn native_widget_requires_windows_landmarks_instead_of_guessing() {
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
            child_placement(&taskbar, TaskbarLandmarks::default(), 3),
            None
        );
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

        assert_eq!(placement.x, 428);
        assert_eq!(placement.width, 240);
    }
}

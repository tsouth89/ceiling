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
    open_on_hover: bool,
}

/// Computes the horizontal position that centers content within an item.
///
/// If the content is wider than the item, returns the item's left edge.
///
/// # Examples
///
/// ```
/// assert_eq!(centered_content_x(10, 100, 40), 40);
/// assert_eq!(centered_content_x(10, 20, 40), 10);
/// ```
fn centered_content_x(item_left: i32, item_width: i32, content_width: i32) -> i32 {
    item_left.saturating_add(item_width.saturating_sub(content_width).max(0) / 2)
}

/// Converts a window style for use by an Explorer child window.

///

/// # Examples

///

/// ```

/// let style = explorer_child_style(0x8000_0000);

/// assert_eq!(style & 0x8000_0000, 0);

/// assert_ne!(style & 0x4000_0000, 0);

/// ```
fn explorer_child_style(style: u32) -> u32 {
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_POPUP: u32 = 0x8000_0000;
    (style & !WS_POPUP) | WS_CHILD
}

/// Determines whether the native taskbar widget is enabled.
///
/// # Examples
///
/// ```
/// let mut settings = codexbar::settings::Settings::default();
/// settings.taskbar_widget_enabled = true;
///
/// assert!(native_mode_enabled(&settings));
/// ```
pub fn native_mode_enabled(settings: &codexbar::settings::Settings) -> bool {
    settings.taskbar_widget_enabled
}

/// Determines whether a provider selected for the native taskbar widget is enabled.
///
/// # Returns
///
/// `true` if at least one preferred provider is enabled, `false` otherwise.
///
/// # Examples
///
/// ```
/// let mut settings = codexbar::settings::Settings::default();
/// settings.float_bar_provider_ids = vec!["codex".to_string()];
/// settings.enabled_providers = vec!["codex".to_string()];
///
/// assert!(native_mode_has_configured_provider(&settings));
/// ```
fn native_mode_has_configured_provider(settings: &codexbar::settings::Settings) -> bool {
    let preferred_ids = if settings.float_bar_provider_ids.is_empty() {
        settings.provider_display_order_names()
    } else {
        settings.float_bar_provider_ids.clone()
    };
    preferred_ids
        .iter()
        .any(|provider_id| settings.enabled_providers.contains(provider_id))
}

/// Determines whether a taskbar layout is eligible for use.
///
/// A layout is eligible when all-monitor mode is enabled or the layout represents the primary taskbar.
///
/// # Examples
///
/// ```
/// let layout = TaskbarLayout {
///     primary: true,
///     ..Default::default()
/// };
///
/// assert!(layout_is_enabled(&layout, false));
/// assert!(layout_is_enabled(&layout, true));
/// ```
fn layout_is_enabled(layout: &TaskbarLayout, all_monitors: bool) -> bool {
    all_monitors || layout.primary
}

/// Collects eligible taskbar handles and their verified widget placements.
///
/// Taskbars are included when enabled for the selected monitor scope and have a valid
/// window handle. A taskbar is added to the placement list only when its layout can
/// accommodate the requested number of providers.
///
/// # Examples
///
/// ```
/// let (discovered, placements) = taskbar_placements(&[], false, 1);
/// assert!(discovered.is_empty());
/// assert!(placements.is_empty());
/// ```
fn taskbar_placements(
    layouts: &[TaskbarLayout],
    all_monitors: bool,
    provider_count: usize,
) -> (Vec<isize>, Vec<(isize, ChildPlacement)>) {
    let mut discovered = Vec::new();
    let mut placements = Vec::new();
    for layout in layouts
        .iter()
        .filter(|layout| layout_is_enabled(layout, all_monitors))
        .filter(|layout| layout.window_handle != 0)
    {
        discovered.push(layout.window_handle);
        if let Some(placement) = child_placement(layout, layout.landmarks, provider_count) {
            placements.push((layout.window_handle, placement));
        }
    }
    (discovered, placements)
}

/// Computes a verified widget placement within a taskbar's available horizontal lane.
///
/// The placement is rejected when the taskbar is vertical, no providers are displayed,
/// required landmarks are invalid, or no obstacle-free gap can fit the widget.
///
/// # Examples
///
/// ```ignore
/// let placement = child_placement(&layout, landmarks, 2);
/// assert!(placement.is_some());
/// ```
///
/// # Arguments
///
/// * `layout` - Taskbar bounds and obstacle rectangles used to determine available space.
/// * `landmarks` - Explorer landmarks that define the verified widget lane.
/// * `provider_count` - Number of provider entries the widget must display.
///
/// # Returns
///
/// A placement relative to the taskbar client area, or `None` when no verified
/// obstacle-free placement can accommodate the providers.
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

/// Installs the native taskbar widget host for the application.
///
/// On non-Windows platforms, this function has no effect.
///
/// # Examples
///
/// ```no_run
/// fn setup(app: &tauri::AppHandle) {
///     install(app);
/// }
/// ```
pub fn install(app: &tauri::AppHandle) {
    #[cfg(windows)]
    windows_host::install(app);
    #[cfg(not(windows))]
    let _ = app;
}

/// Applies the configured native taskbar widget state.
///
/// Enables or refreshes the widget when native mode has a configured provider;
/// otherwise, hides existing widget windows.
///
/// # Examples
///
/// ```no_run
/// # let app: tauri::AppHandle = todo!();
/// # let settings: codexbar::settings::Settings = todo!();
/// apply_state(&app, &settings);
/// ```
pub fn apply_state(app: &tauri::AppHandle, settings: &codexbar::settings::Settings) {
    #[cfg(windows)]
    windows_host::apply_state(app, settings);
    #[cfg(not(windows))]
    let _ = (app, settings);
}

/// Samples the current Explorer taskbar surface color for use as a widget tint.
///
/// # Returns
///
/// A hexadecimal RGB color string when the taskbar color can be sampled, or
/// `None` when sampling is unavailable.
///
/// # Examples
///
/// ```
/// let color = get_taskbar_surface_color();
/// assert!(color.is_none() || color.is_some());
/// ```
///
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
    const WINDOW_TITLE: &str = "Ceiling taskbar widget";

    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
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
    const WM_TIMER: u32 = 0x0113;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    const MA_NOACTIVATE: isize = 3;
    const IDC_ARROW: usize = 32512;
    const TME_LEAVE: u32 = 0x0000_0002;
    const HOVER_TIMER_ID: usize = 0xCE11;
    const HOVER_DWELL_MS: u32 = 150;
    const HOVER_DISMISS_GRACE: std::time::Duration = std::time::Duration::from_millis(180);
    const HOVER_POINTER_POLL: std::time::Duration = std::time::Duration::from_millis(50);
    const TRANSPARENT: i32 = 1;
    const PS_SOLID: i32 = 0;
    const FONT_QUALITY_ANTIALIASED: u32 = 4;
    const GWL_STYLE: i32 = -16;
    // A deliberately uncommon key color. Pixels left in this color are
    // transparent, allowing Explorer's own taskbar material to show through.
    const TRANSPARENT_KEY: u32 = rgb(1, 2, 3);

    #[derive(Debug, Default)]
    struct HostedWidget {
        hwnd: isize,
        taskbar: isize,
    }

    #[derive(Debug, Default)]
    struct HostState {
        widgets: Vec<HostedWidget>,
        model: WidgetModel,
    }

    struct PreparedWidget {
        taskbar: isize,
        placement: ChildPlacement,
    }

    struct PreparedWidgets {
        widgets: Vec<PreparedWidget>,
        discovered_taskbars: Vec<isize>,
        model: WidgetModel,
    }

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static HOST: OnceLock<Mutex<HostState>> = OnceLock::new();
    static CLASS_REGISTERED: OnceLock<bool> = OnceLock::new();
    static RECOVERY_PENDING: AtomicBool = AtomicBool::new(false);
    static HOVER_TRACKING: AtomicBool = AtomicBool::new(false);
    static HOVER_FLYOUT_OPEN: AtomicBool = AtomicBool::new(false);

    /// Installs the native taskbar widget host and starts its recovery and provider-refresh tasks.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// install(&app);
    /// ```
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

    /// Applies the native taskbar widget state for the supplied settings.
    ///
    /// Schedules widget recovery when native mode is enabled with a configured provider;
    /// otherwise, hides existing widget windows.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// apply_state(&app, &settings);
    /// ```
    pub(super) fn apply_state(app: &tauri::AppHandle, settings: &codexbar::settings::Settings) {
        if native_mode_enabled(settings) && native_mode_has_configured_provider(settings) {
            schedule_recovery(app);
        } else {
            hide_existing();
        }
    }

    /// Samples the primary Windows taskbar surface color.
    ///
    /// # Examples
    ///
    /// ```
    /// let color = taskbar_surface_color();
    /// assert!(color.is_none() || color.unwrap().starts_with('#'));
    /// ```
    pub(super) fn taskbar_surface_color() -> Option<String>
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

    /// Schedules a taskbar widget recovery without overlapping an existing recovery.
    ///
    /// Recovery runs in the background, applies successful updates on the main thread,
    /// and preserves the current widget when discovery or preparation fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// schedule_recovery(&app);
    /// ```
    fn schedule_recovery(app: &tauri::AppHandle) {
    fn schedule_recovery(app: &tauri::AppHandle) {
        if RECOVERY_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let prepared = tauri::async_runtime::spawn_blocking(prepare_widgets).await;
            let dispatched = app.run_on_main_thread(move || {
                match prepared {
                    Ok(Ok(prepared)) => {
                        if let Err(error) = apply_prepared(prepared) {
                            tracing::warn!(%error, "Native taskbar widget proof update failed");
                        }
                    }
                    Ok(Err(error)) => {
                        // Start, Search, Widgets, and other Explorer surfaces can
                        // temporarily make UI Automation landmarks unavailable.
                        // Keep the last known healthy child visible rather than
                        // turning a transient discovery miss into user-visible
                        // flicker and a slow rediscovery cycle.
                        tracing::debug!(%error, "Native taskbar widget recovery deferred; preserving the current widget");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Native taskbar discovery worker failed; preserving the current widget");
                    }
                }
                RECOVERY_PENDING.store(false, Ordering::Release);
            });
            if dispatched.is_err() {
                RECOVERY_PENDING.store(false, Ordering::Release);
            }
        });
    }

    /// Prepares native taskbar widgets from the current settings and discovered taskbar layouts.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = prepare_widgets();
    ///
    /// if let Ok(prepared) = result {
    ///     assert!(!prepared.widgets.is_empty());
    /// }
    /// ```
    ///
    /// # Returns
    ///
    /// The prepared widget placements, discovered taskbar handles, and display model.
    ///
    /// An error if native mode is disabled, no providers are available, or no verified
    /// taskbar lane can fit the widget.
    fn prepare_widgets() -> Result<PreparedWidgets, String> {
    fn prepare_widgets() -> Result<PreparedWidgets, String> {
        let settings = codexbar::settings::Settings::load();
        if !native_mode_enabled(&settings) {
            return Err("Native taskbar mode is disabled".to_string());
        }
        let model = widget_model()?;
        if model.providers.is_empty() {
            return Err("No enabled providers are available for the taskbar widget".to_string());
        }
        let layouts = crate::floatbar::taskbar::discover_all();
        let (discovered_taskbars, placements) = taskbar_placements(
            &layouts,
            settings.taskbar_widget_all_monitors,
            model.providers.len(),
        );
        let widgets = placements
            .into_iter()
            .map(|(taskbar, placement)| PreparedWidget { taskbar, placement })
            .collect::<Vec<_>>();
        if widgets.is_empty() {
            return Err("No verified taskbar lane can fit the native widget".to_string());
        }

        Ok(PreparedWidgets {
            widgets,
            discovered_taskbars,
            model,
        })
    }

    /// Applies prepared widget placements to the native taskbar host.
    ///
    /// Updates the hosted widget model, removes widgets for undiscovered taskbars,
    /// creates or reparents widget windows as needed, and applies their current
    /// placement and visibility.
    ///
    /// # Errors
    ///
    /// Returns an error if the host state is poisoned or a widget window cannot be
    /// created.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// apply_prepared(prepared_widgets)?;
    /// # Ok::<(), String>(())
    /// ```
    fn apply_prepared(prepared: PreparedWidgets) -> Result<(), String> {
        let mut state = HOST
            .get_or_init(|| Mutex::new(HostState::default()))
            .lock()
            .map_err(|_| "Native taskbar widget state is poisoned".to_string())?;

        let model_changed = state.model != prepared.model;
        state.model = prepared.model;
        state.widgets.retain(|widget| {
            let keep = prepared.discovered_taskbars.contains(&widget.taskbar);
            if !keep && widget.hwnd != 0 && unsafe { IsWindow(widget.hwnd) } != 0 {
                unsafe { DestroyWindow(widget.hwnd) };
            }
            keep
        });

        for prepared_widget in prepared.widgets {
            let index = state
                .widgets
                .iter()
                .position(|widget| widget.taskbar == prepared_widget.taskbar)
                .unwrap_or_else(|| {
                    state.widgets.push(HostedWidget {
                        hwnd: 0,
                        taskbar: prepared_widget.taskbar,
                    });
                    state.widgets.len() - 1
                });
            let widget = &mut state.widgets[index];
            let window_alive = widget.hwnd != 0 && unsafe { IsWindow(widget.hwnd) } != 0;
            let correctly_parented =
                window_alive && unsafe { GetParent(widget.hwnd) } == prepared_widget.taskbar;
            if !correctly_parented {
                if window_alive {
                    unsafe { DestroyWindow(widget.hwnd) };
                }
                widget.hwnd = unsafe { create_widget(prepared_widget.taskbar)? };
                tracing::info!("Created native Ceiling taskbar widget");
            }

            unsafe {
                SetWindowRgn(widget.hwnd, 0, 1);
                SetWindowPos(
                    widget.hwnd,
                    0,
                    prepared_widget.placement.x,
                    prepared_widget.placement.y,
                    prepared_widget.placement.width,
                    prepared_widget.placement.height,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                );
                ShowWindow(widget.hwnd, SW_SHOWNA);
                if model_changed {
                    InvalidateRect(widget.hwnd, std::ptr::null(), 0);
                }
            }
        }
        Ok(())
    }

    /// Builds the current native taskbar widget model from application state and settings.
    ///
    /// The model includes up to three enabled providers, their usage percentages, compact
    /// window labels, optional inline reset text, theme-appropriate text contrast, and
    /// hover-to-open behavior.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let model = widget_model().expect("application state should be available");
    /// println!("Displaying {} providers", model.providers.len());
    /// ```
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

        // The taskbar surface follows Windows. Manual contrast is retained only
        // for the free-floating bar where the desktop background is unknown.
        let dark_text = system_uses_light_theme();

        Ok(WidgetModel {
            providers,
            dark_text,
            open_on_hover: settings.taskbar_widget_open_on_hover,
        })
    }

    /// Determines whether Windows is configured to use a light system theme.
    
    ///
    
    /// # Examples
    
    ///
    
    /// ```
    
    /// let uses_light_theme = system_uses_light_theme();
    
    /// println!("Light theme enabled: {uses_light_theme}");
    
    /// ```
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

    /// Creates a compact display label from a usage-window label and duration.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compact_window_label(Some("5-hour limit"), None), "5h");
    /// assert_eq!(compact_window_label(None, Some(120)), "2h");
    /// ```
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

    /// Hides all currently hosted taskbar widgets.
    ///
    /// # Examples
    ///
    /// ```
    /// hide_existing();
    /// ```
    fn hide_existing() {
        let Some(host) = HOST.get() else {
            return;
        };
        let Ok(state) = host.try_lock() else {
            return;
        };
        for widget in &state.widgets {
            if widget.hwnd != 0 && unsafe { IsWindow(widget.hwnd) } != 0 {
                unsafe { ShowWindow(widget.hwnd, SW_HIDE) };
            }
        }
    }

    /// Finds the primary Windows taskbar window.
    ///
    /// # Examples
    ///
    /// ```
    /// let taskbar = unsafe { find_primary_taskbar() };
    /// assert!(taskbar.is_some() || taskbar.is_none());
    /// ```
    ///
    /// # Safety
    ///
    /// This function must be called on Windows.
    unsafe fn find_primary_taskbar() -> Option<isize> {
        let class = wide("Shell_TrayWnd");
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        (hwnd != 0).then_some(hwnd)
    }

    /// Creates and attaches a native widget window to a taskbar.
    ///
    /// The returned handle identifies the configured child window. Returns an error if
    /// the window class cannot be registered, the window cannot be created or attached,
    /// or layered composition cannot be enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = unsafe { create_widget(0) };
    /// assert!(result.is_err());
    /// ```
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
        // SetParent intentionally does not update WS_POPUP/WS_CHILD. Microsoft
        // requires changing those bits before attaching a desktop popup to a
        // non-null parent; leaving the popup style in Explorer previously made
        // the host vulnerable to broken input and shell repaint behavior.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
        let child_style = explorer_child_style(style);
        unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, child_style as isize) };
        let applied_style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
        if applied_style & WS_POPUP != 0 || applied_style & WS_CHILD == 0 {
            unsafe { DestroyWindow(hwnd) };
            return Err("Could not apply the taskbar child window style".to_string());
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

    /// Registers the native taskbar widget window class.
    ///
    /// # Examples
    ///
    /// ```
    /// let registered = unsafe { register_class() };
    /// assert!(registered);
    /// ```
    ///
    /// # Returns
    ///
    /// `true` if the window class was registered successfully, `false` otherwise.
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
            cursor: unsafe { LoadCursorW(0, IDC_ARROW as *const u16) },
            background: 0,
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            small_icon: 0,
        };
        unsafe { RegisterClassExW(&wc) != 0 }
    }

    /// Processes window messages for the hosted taskbar widget, including painting,
    /// pointer interaction, hover activation, and cleanup.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = unsafe { widget_window_proc(0, WM_ERASEBKGND, 0, 0) };
    /// assert_eq!(result, 1);
    /// ```
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
                unsafe { SetCursor(LoadCursorW(0, IDC_ARROW as *const u16)) };
                1
            }
            WM_MOUSEMOVE => {
                begin_hover_dwell(hwnd);
                0
            }
            WM_MOUSELEAVE => {
                cancel_hover_dwell(hwnd);
                0
            }
            WM_TIMER if wparam == HOVER_TIMER_ID => {
                unsafe { KillTimer(hwnd, HOVER_TIMER_ID) };
                if hover_open_enabled() {
                    open_flyout(hwnd);
                }
                0
            }
            WM_LBUTTONUP => {
                // A deliberate click owns the interaction until the pointer
                // leaves, so the pending hover timer cannot immediately undo
                // a click-to-close action.
                unsafe { KillTimer(hwnd, HOVER_TIMER_ID) };
                HOVER_FLYOUT_OPEN.store(false, Ordering::Release);
                toggle_flyout(hwnd);
                0
            }
            WM_DESTROY => {
                cancel_hover_dwell(hwnd);
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    /// Determines whether opening the flyout on hover is enabled for the current widget model.
    ///
    /// Returns `false` when the host state is unavailable or cannot be locked.
    ///
    /// # Examples
    ///
    /// ```
    /// let enabled = hover_open_enabled();
    /// assert!(enabled == true || enabled == false);
    /// ```
    fn hover_open_enabled() -> bool {
        HOST.get()
            .and_then(|host| host.try_lock().ok().map(|state| state.model.open_on_hover))
            .unwrap_or(false)
    }

    /// Starts tracking pointer dwell over the widget and schedules hover activation.
    ///
    /// Cancels tracking if mouse-leave or timer registration fails.
    ///
    /// # Examples
    ///
    /// ```
    /// begin_hover_dwell(hwnd);
    /// ```
    fn begin_hover_dwell(hwnd: isize) {
        if !hover_open_enabled()
            || HOVER_TRACKING
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }

        let mut tracking = TrackMouseEventParams {
            size: std::mem::size_of::<TrackMouseEventParams>() as u32,
            flags: TME_LEAVE,
            hwnd_track: hwnd,
            hover_time: 0,
        };
        let leave_armed = unsafe { TrackMouseEvent(&mut tracking) } != 0;
        let timer_armed = unsafe { SetTimer(hwnd, HOVER_TIMER_ID, HOVER_DWELL_MS, None) } != 0;
        if !leave_armed || !timer_armed {
            cancel_hover_dwell(hwnd);
        }
    }

    /// Cancels hover tracking for a widget window and disarms its hover timer.
    ///
    /// # Examples
    ///
    /// ```
    /// HOVER_TRACKING.store(true, Ordering::Release);
    /// cancel_hover_dwell(0);
    /// assert!(!HOVER_TRACKING.load(Ordering::Acquire));
    /// ```
    fn cancel_hover_dwell(hwnd: isize) {
        unsafe { KillTimer(hwnd, HOVER_TIMER_ID) };
        HOVER_TRACKING.store(false, Ordering::Release);
    }

    /// Records the widget's screen rectangle as the flyout's tray anchor.
    ///
    /// The anchor is updated only when the widget rectangle is available and application state can be acquired.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// remember_flyout_anchor(&app, widget_hwnd);
    /// ```
    fn remember_flyout_anchor(app: &tauri::AppHandle, hwnd: isize) {
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
    }

    /// Opens or focuses the flyout associated with a taskbar widget and monitors hover dismissal.
    ///
    /// Does nothing when the application handle is unavailable.
    ///
    /// # Examples
    ///
    /// ```
    /// open_flyout(0);
    /// ```
    fn open_flyout(hwnd: isize) {
        let Some(app) = APP.get().cloned() else {
            return;
        };
        remember_flyout_anchor(&app, hwnd);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = crate::shell::flyout_window::open_or_focus(&app, None) {
                HOVER_FLYOUT_OPEN.store(false, Ordering::Release);
                tracing::warn!(%error, "Could not open native taskbar widget flyout on hover");
                return;
            }
            if !HOVER_FLYOUT_OPEN.swap(true, Ordering::AcqRel) {
                monitor_hover_flyout(app, hwnd).await;
            }
        });
    }

    /// Monitors the pointer and dismisses the hover flyout after it remains outside the widget and flyout.
    ///
    /// # Arguments
    ///
    /// * `widget_hwnd` - Handle of the taskbar widget window.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// monitor_hover_flyout(app, widget_hwnd).await;
    /// ```
    async fn monitor_hover_flyout(app: tauri::AppHandle, widget_hwnd: isize) {
    async fn monitor_hover_flyout(app: tauri::AppHandle, widget_hwnd: isize) {
        let mut outside_since = None;
        loop {
            tokio::time::sleep(HOVER_POINTER_POLL).await;
            if !HOVER_FLYOUT_OPEN.load(Ordering::Acquire) {
                return;
            }

            let Some(pointer) = cursor_position() else {
                continue;
            };
            if point_is_inside_window(widget_hwnd, pointer) || point_is_inside_flyout(&app, pointer)
            {
                outside_since = None;
                continue;
            }

            let since = outside_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() < HOVER_DISMISS_GRACE {
                continue;
            }

            if HOVER_FLYOUT_OPEN.swap(false, Ordering::AcqRel)
                && let Err(error) = crate::shell::flyout_window::hide(&app)
            {
                tracing::warn!(%error, "Could not dismiss native taskbar hover flyout");
            }
            return;
        }
    }

    /// Retrieves the current cursor position in screen coordinates.
    ///
    /// # Examples
    ///
    /// ```
    /// let position = cursor_position();
    /// assert!(position.is_some());
    /// ```
    fn cursor_position() -> Option<WinPoint>
    fn cursor_position() -> Option<WinPoint> {
        let mut point = WinPoint { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut point) } != 0).then_some(point)
    }

    /// Determines whether a screen point lies within a window's bounds.
    
    ///
    
    /// # Examples
    
    ///
    
    /// ```
    
    /// let point = WinPoint { x: 0, y: 0 };
    
    /// assert!(!point_is_inside_window(0, point));
    
    /// ```
    fn point_is_inside_window(hwnd: isize, point: WinPoint) -> bool {
        if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
            return false;
        }
        let mut rect = WinRect::default();
        (unsafe { GetWindowRect(hwnd, (&mut rect as *mut WinRect).cast()) }) != 0
            && point_is_inside_rect(point, &rect)
    }

    /// Determines whether a screen point is within the visible flyout window.
    ///
    /// The hover state is cleared when the flyout is unavailable or no longer visible.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let inside = point_is_inside_flyout(&app, point);
    /// assert!(inside);
    /// ```
    fn point_is_inside_flyout(app: &tauri::AppHandle, point: WinPoint) -> bool {
        let Some(window) = app.get_webview_window(crate::shell::flyout_window::FLYOUT_LABEL) else {
            return false;
        };
        if !window.is_visible().unwrap_or(false) {
            HOVER_FLYOUT_OPEN.store(false, Ordering::Release);
            return false;
        }
        let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else {
            return false;
        };
        let rect = WinRect {
            left: position.x,
            top: position.y,
            right: position.x.saturating_add(size.width as i32),
            bottom: position.y.saturating_add(size.height as i32),
        };
        point_is_inside_rect(point, &rect)
    }

    /// Determines whether a point lies within a rectangle's half-open bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// let rect = WinRect {
    ///     left: 10,
    ///     top: 20,
    ///     right: 30,
    ///     bottom: 40,
    /// };
    ///
    /// assert!(point_is_inside_rect(WinPoint { x: 10, y: 20 }, &rect));
    /// assert!(!point_is_inside_rect(WinPoint { x: 30, y: 40 }, &rect));
    /// ```
    ///
    /// # Parameters
    ///
    /// * `point` - The point to test.
    /// * `rect` - The rectangle defining the bounds.
    ///
    /// # Returns
    ///
    /// `true` if the point is inside the rectangle, `false` otherwise.
    fn point_is_inside_rect(point: WinPoint, rect: &WinRect) -> bool {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    }

    /// Toggles the flyout associated with the native taskbar widget.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// toggle_flyout(widget_hwnd);
    /// ```
    fn toggle_flyout(hwnd: isize) {
        let Some(app) = APP.get().cloned() else {
            return;
        };
        remember_flyout_anchor(&app, hwnd);

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

    /// Paints the widget window with the current provider readouts, labels, icons, and separators.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Called from the widget window procedure for a valid window handle.
    /// unsafe {
    ///     paint_widget(hwnd);
    /// }
    /// ```
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
            let label = provider
                .percent
                .map(|percent| format!("{percent}%"))
                .unwrap_or_else(|| "—".to_string());
            let label = wide_without_nul(&label);
            let label_width = unsafe { text_width(hdc, &label) };
            const ICON_WIDTH: i32 = 16;
            const ICON_TEXT_GAP: i32 = 5;
            let primary_width = ICON_WIDTH
                .saturating_add(ICON_TEXT_GAP)
                .saturating_add(label_width);
            let primary_left = centered_content_x(item_left, item_width, primary_width);
            unsafe {
                draw_provider_icon(
                    hdc,
                    &provider.provider_id,
                    primary_left + ICON_WIDTH / 2,
                    middle - 7,
                    color,
                )
            };
            unsafe {
                SetTextColor(hdc, text_color);
                TextOutW(
                    hdc,
                    primary_left + ICON_WIDTH + ICON_TEXT_GAP,
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
                let detail_width = text_width(hdc, &detail);
                TextOutW(
                    hdc,
                    centered_content_x(item_left, item_width, detail_width),
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

    /// Draws the icon for a provider at the specified position and color.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Requires a valid Win32 device context.
    /// unsafe {
    ///     draw_provider_icon(hdc, "codex", 16, 16, 0x0000FF);
    /// }
    /// ```
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

    /// Draws a 16×16 monochrome icon mask at the specified position.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let rows = [0b0001u16; 16];
    /// unsafe {
    ///     draw_icon_mask(hdc, &rows, 10, 10, 0x00FF00);
    /// }
    /// ```
    ///
    /// # Safety
    ///
    /// `hdc` must be a valid device context handle, and the target device context
    /// must remain valid for the duration of the call.
    ///
    /// `rows` supplies one 16-bit pixel mask for each row. Set bits are drawn using
    /// `color`.
    ///
    /// # Parameters
    ///
    /// * `hdc` - Device context handle to draw into.
    /// * `rows` - Sixteen row masks describing the icon pixels.
    /// * `x` - Left coordinate of the icon.
    /// * `y` - Top coordinate of the icon.
    /// * `color` - Pixel color.
    unsafe fn draw_icon_mask(hdc: isize, rows: &[u16; 16], x: i32, y: i32, color: u32) {
    unsafe fn draw_icon_mask(hdc: isize, rows: &[u16; 16], x: i32, y: i32, color: u32) {
        for (row_index, row) in rows.iter().copied().enumerate() {
            for column in 0..16 {
                if row & (1 << column) != 0 {
                    unsafe { SetPixelV(hdc, x + column, y + row_index as i32, color) };
                }
            }
        }
    }

    /// Selects the display color associated with a provider identifier.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(provider_color("claude"), rgb(216, 116, 75));
    /// assert_eq!(provider_color("unknown"), rgb(204, 211, 220));
    /// ```
    fn provider_color(provider_id: &str) -> u32 {
        match provider_id {
            "claude" => rgb(216, 116, 75),
            "cursor" => rgb(15, 201, 181),
            "codex" => rgb(64, 196, 222),
            _ => rgb(204, 211, 220),
        }
    }

    /// Packs red, green, and blue channel values into a Win32 color value.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(rgb(0x12, 0x56, 0x34), 0x0034_5612);
    /// ```
    const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
        red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
    }

    /// Encodes a string as a null-terminated UTF-16 sequence.
    ///
    /// # Examples
    ///
    /// ```
    /// let encoded = wide("Hi");
    /// assert_eq!(encoded, vec![72, 105, 0]);
    /// ```
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Encodes a string as UTF-16 code units without a trailing null terminator.
    ///
    /// # Examples
    ///
    /// ```
    /// let encoded = wide_without_nul("Hi");
    /// assert_eq!(encoded, vec![72, 105]);
    /// ```
    fn wide_without_nul(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    /// Measures the rendered width of UTF-16 text using the specified device context.
    ///
    /// Falls back to an estimate of seven pixels per UTF-16 code unit when the text is empty or its measured width cannot be obtained.
    ///
    /// # Safety
    ///
    /// The device context handle must be valid when `text` is non-empty.
    ///
    /// # Examples
    ///
    /// ```
    /// let width = unsafe { text_width(0, &[]) };
    /// assert_eq!(width, 0);
    /// ```
    unsafe fn text_width(hdc: isize, text: &[u16]) -> i32 {
        let mut size = WinSize::default();
        if text.is_empty()
            || unsafe { GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size) }
                == 0
        {
            return i32::try_from(text.len()).unwrap_or(0).saturating_mul(7);
        }
        size.cx.max(0)
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
    struct WinSize {
        cx: i32,
        cy: i32,
    }

    #[repr(C)]
    struct TrackMouseEventParams {
        size: u32,
        flags: u32,
        hwnd_track: isize,
        hover_time: u32,
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
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
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
        fn TrackMouseEvent(event: *mut TrackMouseEventParams) -> i32;
        fn SetTimer(
            hwnd: isize,
            event_id: usize,
            interval_ms: u32,
            callback: Option<unsafe extern "system" fn(isize, u32, usize, u32)>,
        ) -> usize;
        fn KillTimer(hwnd: isize, event_id: usize) -> i32;
        fn BeginPaint(hwnd: isize, paint: *mut PaintStruct) -> isize;
        fn EndPaint(hwnd: isize, paint: *const PaintStruct) -> i32;
        fn GetClientRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut std::ffi::c_void) -> i32;
        fn GetCursorPos(point: *mut WinPoint) -> i32;
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
        fn GetTextExtentPoint32W(
            hdc: isize,
            text: *const u16,
            count: i32,
            size: *mut WinSize,
        ) -> i32;
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
    use codexbar::settings::Settings;

    #[test]
    fn provider_content_is_centered_inside_its_segment() {
        assert_eq!(centered_content_x(92, 92, 48), 114);
        assert_eq!(centered_content_x(184, 92, 60), 200);
    }

    #[test]
    fn oversized_provider_content_stays_at_the_segment_start() {
        assert_eq!(centered_content_x(92, 72, 90), 92);
    }

    #[test]
    fn explorer_parenting_replaces_popup_style_with_child_style() {
        const WS_VISIBLE: u32 = 0x1000_0000;
        const WS_CHILD: u32 = 0x4000_0000;
        const WS_POPUP: u32 = 0x8000_0000;
        let style = explorer_child_style(WS_VISIBLE | WS_POPUP);

        assert_eq!(style & WS_POPUP, 0);
        assert_eq!(style & WS_CHILD, WS_CHILD);
        assert_eq!(style & WS_VISIBLE, WS_VISIBLE);
    }

    fn layout(bounds: Rect, obstacles: Vec<Rect>) -> TaskbarLayout {
        TaskbarLayout {
            window_handle: 1,
            bounds,
            monitor_bounds: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            obstacles,
            landmarks: TaskbarLandmarks::default(),
            primary: true,
        }
    }

    #[test]
    fn multi_monitor_setting_includes_secondary_taskbars_only_when_enabled() {
        let primary = TaskbarLayout {
            primary: true,
            ..layout(
                Rect {
                    left: 0,
                    top: 1392,
                    right: 2560,
                    bottom: 1440,
                },
                Vec::new(),
            )
        };
        let secondary = TaskbarLayout {
            window_handle: 2,
            primary: false,
            ..layout(
                Rect {
                    left: -1920,
                    top: 1032,
                    right: 0,
                    bottom: 1080,
                },
                Vec::new(),
            )
        };

        assert!(layout_is_enabled(&primary, false));
        assert!(!layout_is_enabled(&secondary, false));
        assert!(layout_is_enabled(&primary, true));
        assert!(layout_is_enabled(&secondary, true));
    }

    #[test]
    fn mixed_resolution_taskbars_receive_independent_local_placements() {
        let primary_bounds = Rect {
            left: 0,
            top: 1392,
            right: 2560,
            bottom: 1440,
        };
        let secondary_bounds = Rect {
            left: -1920,
            top: 1032,
            right: 0,
            bottom: 1080,
        };
        let primary = TaskbarLayout {
            window_handle: 1,
            landmarks: landmarks(
                Rect {
                    left: 0,
                    top: 1392,
                    right: 160,
                    bottom: 1440,
                },
                Rect {
                    left: 1120,
                    top: 1392,
                    right: 1168,
                    bottom: 1440,
                },
            ),
            ..layout(primary_bounds, Vec::new())
        };
        let secondary = TaskbarLayout {
            window_handle: 2,
            bounds: secondary_bounds,
            monitor_bounds: Rect {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            },
            landmarks: TaskbarLandmarks {
                widgets: None,
                start: Some(Rect {
                    left: -1040,
                    top: 1032,
                    right: -992,
                    bottom: 1080,
                }),
            },
            primary: false,
            ..layout(secondary_bounds, Vec::new())
        };

        let (discovered, placements) = taskbar_placements(&[primary, secondary], true, 3);
        assert_eq!(discovered, vec![1, 2]);
        assert_eq!(placements.len(), 2);
        assert!(placements.iter().all(|(_, placement)| placement.x >= 0));
        assert!(
            placements
                .iter()
                .all(|(_, placement)| placement.height == 48)
        );
    }

    fn landmarks(widgets: Rect, start: Rect) -> TaskbarLandmarks {
        TaskbarLandmarks {
            widgets: Some(widgets),
            start: Some(start),
        }
    }

    #[test]
    fn native_mode_requires_at_least_one_enabled_selected_provider() {
        let mut settings = Settings {
            float_bar_enabled: true,
            float_bar_style: "taskbar".to_string(),
            enabled_providers: ["codex".to_string()].into_iter().collect(),
            float_bar_provider_ids: vec!["claude".to_string()],
            ..Settings::default()
        };

        assert!(!native_mode_has_configured_provider(&settings));
        settings.float_bar_provider_ids = vec!["codex".to_string()];
        assert!(native_mode_has_configured_provider(&settings));
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

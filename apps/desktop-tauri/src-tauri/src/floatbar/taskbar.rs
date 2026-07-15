//! Windows taskbar discovery for the floating bar.

use super::placement::{Point, Rect, place_in_taskbar};

const EDGE_PADDING: i32 = 8;
const OBSTACLE_CLEARANCE: i32 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskbarLayout {
    pub bounds: Rect,
    pub obstacles: Vec<Rect>,
}

impl TaskbarLayout {
    pub fn preferred_anchor(&self) -> Point {
        if self.bounds.width() >= self.bounds.height() {
            Point {
                // Leave a taskbar-height-scaled lane for Start/Widgets on a
                // first run. Actual child-window obstacles refine this.
                x: self.bounds.left + self.bounds.height().saturating_mul(10) / 3,
                y: self.bounds.top,
            }
        } else {
            Point {
                x: self.bounds.left,
                y: self.bounds.top + self.bounds.width().saturating_mul(2),
            }
        }
    }

    pub fn place(&self, widget_width: i32, widget_height: i32, preferred: Point) -> Option<Point> {
        place_in_taskbar(
            self.bounds,
            &self.obstacles,
            widget_width,
            widget_height,
            preferred,
            EDGE_PADDING,
            OBSTACLE_CLEARANCE,
        )
    }
}

#[cfg(not(windows))]
pub fn discover() -> Option<TaskbarLayout> {
    None
}

#[cfg(windows)]
pub fn discover() -> Option<TaskbarLayout> {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    impl From<WinRect> for Rect {
        fn from(value: WinRect) -> Self {
            Self {
                left: value.left,
                top: value.top,
                right: value.right,
                bottom: value.bottom,
            }
        }
    }

    struct EnumContext {
        taskbar: Rect,
        obstacles: Vec<Rect>,
    }

    unsafe extern "system" fn collect_child(hwnd: isize, lparam: isize) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let context = &mut *(lparam as *mut EnumContext);
            let mut raw = WinRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(hwnd, (&mut raw as *mut WinRect).cast()) == 0 {
                return 1;
            }
            let rect = Rect::from(raw);
            let width = rect.width();
            let height = rect.height();
            let horizontal = context.taskbar.width() >= context.taskbar.height();
            let taskbar_major = if horizontal {
                context.taskbar.width()
            } else {
                context.taskbar.height()
            };
            let rect_major = if horizontal { width } else { height };
            let rect_cross = if horizontal { height } else { width };
            let taskbar_cross = if horizontal {
                context.taskbar.height()
            } else {
                context.taskbar.width()
            };

            // Ignore invalid rectangles and shell composition containers that
            // span almost the complete taskbar. Smaller visible children are
            // useful obstacle hints on both classic and modern taskbars.
            if width > 0
                && height > 0
                && rect_major < taskbar_major.saturating_mul(3) / 4
                && rect_cross <= taskbar_cross.saturating_mul(2)
                && rect.left < context.taskbar.right
                && rect.right > context.taskbar.left
                && rect.top < context.taskbar.bottom
                && rect.bottom > context.taskbar.top
            {
                context.obstacles.push(rect);
            }
            1
        }
    }

    unsafe {
        let shell_class = wide("Shell_TrayWnd");
        let tray_class = wide("TrayNotifyWnd");
        let shell = FindWindowW(shell_class.as_ptr(), std::ptr::null());
        if shell == 0 {
            return None;
        }
        let tray = FindWindowExW(shell, 0, tray_class.as_ptr(), std::ptr::null());

        let mut shell_rect = WinRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(shell, (&mut shell_rect as *mut WinRect).cast()) == 0 {
            return None;
        }
        let bounds = Rect::from(shell_rect);
        if bounds.width() <= 0 || bounds.height() <= 0 {
            return None;
        }

        let mut context = EnumContext {
            taskbar: bounds,
            obstacles: Vec::new(),
        };
        EnumChildWindows(
            shell,
            Some(collect_child),
            (&mut context as *mut EnumContext).cast::<c_void>() as isize,
        );

        // Always reserve the notification area even if child enumeration was
        // restricted by a shell replacement.
        if tray != 0 {
            let mut tray_rect = WinRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(tray, (&mut tray_rect as *mut WinRect).cast()) != 0 {
                context.obstacles.push(Rect::from(tray_rect));
            }
        }

        context
            .obstacles
            .sort_by_key(|rect| (rect.left, rect.top, rect.right, rect.bottom));
        context.obstacles.dedup();
        Some(TaskbarLayout {
            bounds,
            obstacles: context.obstacles,
        })
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
    fn FindWindowExW(
        parent: isize,
        child_after: isize,
        class_name: *const u16,
        window_name: *const u16,
    ) -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut std::ffi::c_void) -> i32;
    fn EnumChildWindows(
        parent: isize,
        callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
        lparam: isize,
    ) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
}

//! Windows only: the run's progress on the window's taskbar button, as a
//! download shows it.
use windows::Win32::{
    Foundation::HWND,
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    },
    UI::Shell::{
        ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TaskbarList,
    },
};

/// What the taskbar button shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Off,
    /// Working, with no size to measure.
    Busy,
    /// Working, this many thousandths done.
    Share(u16),
    Failed,
}

pub(crate) struct Taskbar {
    list: ITaskbarList3,
    window: HWND,
    shown: State,
}

impl Taskbar {
    /// The taskbar button of `frame`'s window, when Windows offers one.
    pub(crate) fn of(frame: &eframe::Frame) -> Option<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(handle) = frame.window_handle().ok()?.as_raw() else {
            return None;
        };
        let window = HWND(handle.hwnd.get() as *mut _);
        // SAFETY: the UI thread joins (or is already in) a single-threaded
        // apartment; the taskbar object is made and used on this thread.
        let list: ITaskbarList3 = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?
        };
        // SAFETY: a taskbar object just made, on its thread.
        unsafe { list.HrInit() }.ok()?;
        Some(Self {
            list,
            window,
            shown: State::Off,
        })
    }

    /// Show `state`; nothing to do when it is shown already.
    pub(crate) fn show(&mut self, state: State) {
        if state == self.shown {
            return;
        }
        self.shown = state;
        const SCALE: u64 = 1000;
        // SAFETY: calls on the taskbar object, on its thread, for this
        // process's own window. A refused call leaves the button as it was.
        unsafe {
            let _ = match state {
                State::Off => self.list.SetProgressState(self.window, TBPF_NOPROGRESS),
                State::Busy => self.list.SetProgressState(self.window, TBPF_INDETERMINATE),
                State::Share(done) => self
                    .list
                    .SetProgressState(self.window, TBPF_NORMAL)
                    .and_then(|()| {
                        self.list
                            .SetProgressValue(self.window, u64::from(done).min(SCALE), SCALE)
                    }),
                State::Failed => self
                    .list
                    .SetProgressState(self.window, TBPF_ERROR)
                    .and_then(|()| self.list.SetProgressValue(self.window, SCALE, SCALE)),
            };
        }
    }
}

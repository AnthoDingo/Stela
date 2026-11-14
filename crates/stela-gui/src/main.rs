//! `stela-gui`: a window over `stela`'s commands. It builds a command line
//! from a form and runs it in this process through `stela_cli::embed`,
//! showing what it prints. Every check stays in `stela`; the window only
//! spells the options.
// A release build opens no console window beside its own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, TextStyle, vec2,
};
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stela_cli::embed;

#[cfg(windows)]
mod taskbar;

const ICON: &[u8] = include_bytes!("../../../assets/stela-icon.png");

/// The logo's colors (`assets/stela-icon.svg`).
mod palette {
    use eframe::egui::Color32;
    pub const BACKGROUND: Color32 = Color32::from_rgb(0x14, 0x15, 0x18);
    pub const PANEL: Color32 = Color32::from_rgb(0x1B, 0x1C, 0x20);
    pub const CARD: Color32 = Color32::from_rgb(0x24, 0x26, 0x2B);
    pub const HOVER: Color32 = Color32::from_rgb(0x2E, 0x30, 0x36);
    pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x36, 0x3D);
    pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEE);
    pub const MUTED: Color32 = Color32::from_rgb(0x9A, 0xA0, 0xAA);
    pub const TEAL: Color32 = Color32::from_rgb(0x5F, 0xE3, 0xC8);
    pub const BLUE: Color32 = Color32::from_rgb(0x2F, 0x6F, 0xE0);
    pub const AMBER: Color32 = Color32::from_rgb(0xFF, 0xC4, 0x6B);
    pub const RED: Color32 = Color32::from_rgb(0xE2, 0x52, 0x2A);
}
use palette::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Status,
    Create,
    Update,
    Repair,
    Settings,
    Log,
}

impl Action {
    const ALL: [Self; 6] = [
        Self::Status,
        Self::Create,
        Self::Update,
        Self::Repair,
        Self::Settings,
        Self::Log,
    ];

    fn word(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Create => "create",
            Self::Update => "update",
            Self::Repair => "repair",
            Self::Settings => "settings",
            Self::Log => "log",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Create => "Create",
            Self::Update => "Update",
            Self::Repair => "Repair",
            Self::Settings => "Settings",
            Self::Log => "Boot logs",
        }
    }

    /// The commands that run long enough to show their progress, with
    /// their messages folded away.
    fn long(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Repair)
    }

    /// `stela --help`'s line for the command.
    fn about(self) -> &'static str {
        match self {
            Self::Status => "Show what is on a stick and whether it is healthy; changes nothing",
            Self::Create => "Erase a stick and make a macOS installer on it",
            Self::Update => "Bring a stick to this version of Stela",
            Self::Repair => "Fix what is damaged or missing; finish a stopped create or update",
            Self::Settings => "Change a stick's Wi-Fi and network addresses",
            Self::Log => "Save the boot logs for a bug report",
        }
    }
}

/// The form: each field is one option, empty when not given.
struct Form {
    action: Action,
    stick: String,
    dry_run: bool,
    full: bool,
    installer: &'static str,
    macos: String,
    ipsw: String,
    wifi: String,
    passphrase: String,
    confirm_disk: String,
    ipv4: String,
    gateway: String,
    dns: String,
    output: String,
    /// `create`: download macOS to `image_cache` first and keep it there,
    /// where every later `create` finds it.
    keep_image: bool,
    image_cache: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            action: Action::Status,
            stick: String::new(),
            dry_run: false,
            full: false,
            installer: "",
            macos: String::new(),
            ipsw: String::new(),
            wifi: String::new(),
            passphrase: String::new(),
            confirm_disk: String::new(),
            ipv4: String::new(),
            gateway: String::new(),
            dns: String::new(),
            output: "log.txt".into(),
            keep_image: true,
            image_cache: String::new(),
        }
    }
}

impl Form {
    /// The arguments after `stela`. The passphrase is not among them: it
    /// goes through a file, as `--wifi-passphrase-file`.
    fn args(&self) -> Vec<String> {
        let mut args = vec![self.action.word().to_owned()];
        let mut option = |name: &str, value: &str| {
            let value = value.trim();
            if !value.is_empty() {
                args.push(format!("--{name}"));
                args.push(value.to_owned());
            }
        };
        match self.action {
            Action::Status => {}
            Action::Create => {
                option("installer", self.installer);
                option("macos", &self.macos);
                option("ipsw", &self.ipsw);
                option("wifi", &self.wifi);
                option("confirm-disk", &self.confirm_disk);
                // Only a full (offline) installer holds the whole image; an
                // IPSW given is used as it is.
                if self.keep_image && self.installer != "online" && self.ipsw.trim().is_empty() {
                    option("download-dir", &self.image_cache);
                }
            }
            Action::Update => {
                option("macos", &self.macos);
                option("ipsw", &self.ipsw);
            }
            Action::Repair => option("ipsw", &self.ipsw),
            Action::Settings => {
                option("wifi", &self.wifi);
                option("ipv4", &self.ipv4);
                option("gateway", &self.gateway);
                option("dns", &self.dns);
            }
            Action::Log => option("output", &self.output),
        }
        if self.full && matches!(self.action, Action::Status | Action::Repair) {
            args.push("--full".into());
        }
        if !self.stick.trim().is_empty() {
            args.push(self.stick.trim().to_owned());
        }
        if self.dry_run {
            args.push("--dry-run".into());
        }
        // No terminal answers stela's questions; the form already did.
        args.push("--yes".into());
        args
    }

    fn takes_passphrase(&self) -> bool {
        matches!(self.action, Action::Create | Action::Settings) && !self.passphrase.is_empty()
    }
}

#[derive(Default)]
struct Run {
    lines: Vec<String>,
    running: bool,
    /// Whether the last run succeeded, once it ended.
    succeeded: Option<bool>,
    /// The command the last run was, when it started and when it ended.
    action: Option<Action>,
    started: Option<Instant>,
    ended: Option<Instant>,
}

struct App {
    form: Form,
    run: Arc<Mutex<Run>>,
    logo: egui::TextureHandle,
    /// The removable disks plugged in, or why they could not be listed.
    sticks: Result<Vec<embed::Stick>, String>,
    /// The macOS images kept on this computer, with their sizes.
    images: Vec<(std::path::PathBuf, u64)>,
    administrator: bool,
    /// The last step the running command reported, kept once it ends.
    step: Option<embed::Progress>,
    /// Whether a run was going at the last frame.
    was_running: bool,
    /// The window's taskbar button, once looked up (Windows).
    #[cfg(windows)]
    taskbar: Option<Option<taskbar::Taskbar>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme(&cc.egui_ctx);
        let image = eframe::icon_data::from_png_bytes(ICON).unwrap_or_default();
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            ),
            egui::TextureOptions {
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..egui::TextureOptions::LINEAR
            },
        );
        let run = Arc::<Mutex<Run>>::default();
        if let Err(error) = capture(Arc::clone(&run), cc.egui_ctx.clone()) {
            lock(&run).lines.push(format!(
                "error: stela's messages cannot be shown here: {error}"
            ));
        }
        Self {
            form: Form {
                image_cache: embed::image_cache().display().to_string(),
                ..Form::default()
            },
            images: embed::cached_images(),
            run,
            logo,
            sticks: list_sticks(),
            administrator: embed::is_administrator(),
            step: None,
            was_running: false,
            #[cfg(windows)]
            taskbar: None,
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.step = None;
        let args = self.form.args();
        let passphrase = self
            .form
            .takes_passphrase()
            .then(|| self.form.passphrase.clone());
        let run = Arc::clone(&self.run);
        let ctx = ctx.clone();
        {
            let mut run = lock(&run);
            run.running = true;
            run.succeeded = None;
            run.action = Some(self.form.action);
            run.started = Some(Instant::now());
            run.ended = None;
            run.lines = vec![format!("$ stela {}", args.join(" "))];
        }
        std::thread::spawn(move || {
            // What stela prints reaches the panel through `capture`.
            let succeeded = match std::panic::catch_unwind(move || embed::run(&args, passphrase)) {
                Ok(Ok(())) => true,
                Ok(Err(error)) => {
                    eprintln!("error: {error:#}");
                    false
                }
                Err(_) => {
                    eprintln!("error: stela stopped unexpectedly");
                    false
                }
            };
            let mut state = lock(&run);
            state.running = false;
            state.succeeded = Some(succeeded);
            state.ended = Some(Instant::now());
            ctx.request_repaint();
        });
    }
}

fn list_sticks() -> Result<Vec<embed::Stick>, String> {
    embed::sticks().map_err(|error| format!("{error:#}"))
}

fn lock(run: &Mutex<Run>) -> std::sync::MutexGuard<'_, Run> {
    run.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Send this process's stdout and stderr, where stela prints, to the
/// output panel, line by line.
fn capture(run: Arc<Mutex<Run>>, ctx: egui::Context) -> std::io::Result<()> {
    let (reader, writer) = std::io::pipe()?;
    redirect(writer)?;
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            lock(&run).lines.push(line);
            ctx.request_repaint();
        }
    });
    Ok(())
}

#[cfg(windows)]
fn redirect(writer: std::io::PipeWriter) -> std::io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    // Open for the life of the process; Rust's stdout and stderr look the
    // standard handles up at each write.
    let handle = writer.into_raw_handle();
    for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: `handle` is the open write end of a pipe, never closed.
        if unsafe { SetStdHandle(id, handle) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn redirect(writer: std::io::PipeWriter) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: duplicates an open descriptor onto a standard one.
        if unsafe { libc::dup2(writer.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Start this program again with administrator rights, through the UAC
/// dialog; false when it did not start.
#[cfg(windows)]
fn restart_as_administrator() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let wide = |text: &std::ffi::OsStr| text.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("runas".as_ref()), wide(exe.as_os_str()));
    // SAFETY: both strings end in NUL and outlive the call.
    let instance = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as a value above 32.
    instance as usize > 32
}

/// Dark visuals in the logo's colors, with roomier spacing.
fn theme(ctx: &egui::Context) {
    let round = CornerRadius::same(5);
    ctx.style_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(24.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Button, FontId::proportional(14.5)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(10.0, 10.0);
        style.spacing.button_padding = vec2(14.0, 7.0);
        style.spacing.interact_size.y = 32.0;
        let visuals = &mut style.visuals;
        *visuals = egui::Visuals::dark();
        visuals.panel_fill = PANEL;
        visuals.window_fill = CARD;
        visuals.extreme_bg_color = BACKGROUND;
        visuals.faint_bg_color = CARD;
        visuals.hyperlink_color = TEAL;
        visuals.warn_fg_color = AMBER;
        visuals.error_fg_color = RED;
        visuals.selection.bg_fill = BLUE;
        visuals.selection.stroke = Stroke::new(1.0_f32, TEAL);
        visuals.text_cursor.stroke = Stroke::new(2.0_f32, TEAL);
        let widgets = &mut visuals.widgets;
        for (state, fill) in [
            (&mut widgets.noninteractive, PANEL),
            (&mut widgets.inactive, CARD),
            (&mut widgets.hovered, HOVER),
            (&mut widgets.active, HOVER),
            (&mut widgets.open, HOVER),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = round;
            state.bg_stroke = Stroke::new(1.0_f32, BORDER);
            state.fg_stroke = Stroke::new(1.0_f32, TEXT);
            state.expansion = 0.0;
        }
        widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        widgets.hovered.bg_stroke = Stroke::new(1.0_f32, TEAL.gamma_multiply(0.6));
        widgets.active.bg_stroke = Stroke::new(1.0_f32, TEAL);
    });
}

fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(MUTED));
}

fn field(ui: &mut egui::Ui, name: &str, value: &mut String, hint: &str) {
    label(ui, name);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(360.0)
            .margin(vec2(10.0, 7.0)),
    );
    ui.end_row();
}

fn secret(ui: &mut egui::Ui, value: &mut String) {
    label(ui, "Passphrase");
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text("stored in plaintext on the stick")
            .desired_width(360.0)
            .margin(vec2(10.0, 7.0)),
    );
    ui.end_row();
}

/// `create`'s choice to keep macOS on this computer, and what it keeps.
fn image_cache(ui: &mut egui::Ui, form: &mut Form, images: &[(std::path::PathBuf, u64)]) {
    let applies = form.installer != "online" && form.ipsw.trim().is_empty();
    label(ui, "Keep on this PC");
    ui.add_enabled_ui(applies, |ui| {
        ui.checkbox(
            &mut form.keep_image,
            "Download macOS to this PC first, and reuse it for the next sticks",
        )
    });
    ui.end_row();
    let note = match images {
        [] if applies && form.keep_image => {
            format!("About 26 GB, kept in {}", form.image_cache)
        }
        [] => String::new(),
        found => {
            let names: Vec<String> = found
                .iter()
                .map(|(path, bytes)| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    format!("{name} ({})", gigabytes(*bytes))
                })
                .collect();
            format!(
                "On this PC: {}. A matching image is copied, not downloaded.",
                names.join(", ")
            )
        }
    };
    if !note.is_empty() {
        ui.label("");
        ui.label(RichText::new(note).small().color(MUTED));
        ui.end_row();
    }
}

fn describe(stick: &embed::Stick) -> String {
    let stela = if stick.stela { " · Stela" } else { "" };
    format!("{} · {} · {}{stela}", stick.name, stick.size, stick.path)
}

/// The stick a command acts on, from the removable disks plugged in; true
/// when they should be listed again.
fn stick_picker(
    ui: &mut egui::Ui,
    form: &mut Form,
    sticks: &Result<Vec<embed::Stick>, String>,
) -> bool {
    let automatic = match form.action {
        Action::Create => "Choose the stick to erase",
        _ => "The only Stela stick plugged in",
    };
    let listed = sticks.as_deref().unwrap_or_default();
    let selected = match listed.iter().find(|stick| stick.path == form.stick) {
        Some(stick) => describe(stick),
        None if form.stick.is_empty() => automatic.to_owned(),
        None => form.stick.clone(),
    };
    label(ui, "Stick");
    let refresh = ui
        .horizontal(|ui| {
            egui::ComboBox::from_id_salt("stick")
                .width(360.0)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if form.action != Action::Create {
                        ui.selectable_value(&mut form.stick, String::new(), automatic);
                    }
                    for stick in listed {
                        ui.selectable_value(&mut form.stick, stick.path.clone(), describe(stick));
                    }
                });
            ui.button("Refresh")
                .on_hover_text("List the sticks again")
                .clicked()
        })
        .inner;
    ui.end_row();
    let note = match sticks {
        Err(error) => Some((format!("The disks cannot be listed: {error}"), RED)),
        Ok(found) if found.is_empty() => Some((
            "No removable disk found. Plug the stick in, then Refresh.".to_owned(),
            AMBER,
        )),
        Ok(_) => None,
    };
    if let Some((text, color)) = note {
        ui.label("");
        ui.label(RichText::new(text).small().color(color));
        ui.end_row();
    }
    refresh
}

/// One command in the side panel; true when clicked.
fn nav(ui: &mut egui::Ui, action: Action, selected: bool) -> bool {
    let response = egui::Frame::new()
        .fill(if selected {
            TEAL.gamma_multiply(0.12)
        } else {
            Color32::TRANSPARENT
        })
        .stroke(if selected {
            Stroke::new(1.0_f32, TEAL.gamma_multiply(0.5))
        } else {
            Stroke::NONE
        })
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            let color = if selected { TEAL } else { TEXT };
            ui.label(RichText::new(action.title()).strong().color(color));
            ui.label(RichText::new(action.about()).small().color(MUTED));
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

/// The state of the last run, as a colored pill.
fn badge(ui: &mut egui::Ui, run: &Run) {
    let (text, color) = match (run.running, run.succeeded) {
        (true, _) => ("Running", TEAL),
        (false, Some(true)) => ("Done", TEAL),
        (false, Some(false)) => ("Failed", RED),
        (false, None) => ("Ready", MUTED),
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if run.running {
                    ui.add(egui::Spinner::new().size(12.0).color(color));
                }
                ui.label(RichText::new(text).small().strong().color(color));
            });
        });
}

fn line_color(line: &str) -> Color32 {
    let lower = line.to_ascii_lowercase();
    if line.starts_with('$') {
        TEAL
    } else if lower.starts_with("error") || lower.contains(" error:") {
        RED
    } else if lower.starts_with("warning") {
        AMBER
    } else {
        Color32::from_rgb(0xC9, 0xCD, 0xD4)
    }
}

impl App {
    fn sidebar(&mut self, ctx: &egui::Context, running: bool) {
        egui::SidePanel::left("commands")
            .resizable(false)
            .exact_width(270.0)
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(40.0, 40.0)));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new("Stela").size(22.0).strong().color(TEXT));
                        ui.label(RichText::new("macOS installer stick").small().color(MUTED));
                    });
                });
                ui.add_space(18.0);
                ui.add_enabled_ui(!running, |ui| {
                    for action in Action::ALL {
                        if nav(ui, action, self.form.action == action) {
                            self.form.action = action;
                        }
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(MUTED),
                    );
                });
            });
    }

    fn options(&mut self, ui: &mut egui::Ui) {
        let form = &mut self.form;
        let mut refresh = false;
        egui::Grid::new("options")
            .num_columns(2)
            .spacing(vec2(18.0, 12.0))
            .show(ui, |ui| {
                if form.action == Action::Log {
                    field(ui, "Path", &mut form.stick, "every disk when empty");
                } else {
                    refresh = stick_picker(ui, form, &self.sticks);
                }
                match form.action {
                    Action::Status | Action::Repair => {
                        label(ui, "Thorough");
                        ui.checkbox(&mut form.full, "Read every macOS piece (slow)");
                        ui.end_row();
                        if form.action == Action::Repair {
                            field(ui, "IPSW", &mut form.ipsw, "downloaded when empty");
                        }
                    }
                    Action::Create => {
                        label(ui, "Installer");
                        ui.horizontal(|ui| {
                            for (value, text) in [
                                ("", "Automatic"),
                                ("offline", "Offline"),
                                ("online", "Online"),
                            ] {
                                ui.selectable_value(&mut form.installer, value, text);
                            }
                        });
                        ui.end_row();
                        field(ui, "macOS", &mut form.macos, "latest tested");
                        field(ui, "IPSW", &mut form.ipsw, "downloaded when empty");
                        image_cache(ui, form, &self.images);
                        field(ui, "Wi-Fi", &mut form.wifi, "network name, or none");
                        secret(ui, &mut form.passphrase);
                        field(
                            ui,
                            "Confirm disk",
                            &mut form.confirm_disk,
                            "the stick's model or size, e.g. 64GB",
                        );
                    }
                    Action::Update => {
                        field(ui, "macOS", &mut form.macos, "unchanged");
                        field(ui, "IPSW", &mut form.ipsw, "downloaded when empty");
                    }
                    Action::Settings => {
                        field(ui, "Wi-Fi", &mut form.wifi, "network name, or none");
                        secret(ui, &mut form.passphrase);
                        field(ui, "IPv4", &mut form.ipv4, "ADDRESS/PREFIX, or auto");
                        field(ui, "Gateway", &mut form.gateway, "with IPv4");
                        field(
                            ui,
                            "DNS",
                            &mut form.dns,
                            "up to 3, comma-separated, or auto",
                        );
                    }
                    Action::Log => field(ui, "Save to", &mut form.output, "log.txt"),
                }
                label(ui, "Dry run");
                ui.checkbox(&mut form.dry_run, "Show what would change, change nothing");
                ui.end_row();
            });
        if refresh {
            self.sticks = list_sticks();
            self.images = embed::cached_images();
        }
    }

    /// What this process may not do without administrator rights, and how
    /// to get them.
    fn rights_banner(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.administrator {
            return;
        }
        egui::Frame::new()
            .fill(AMBER.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0_f32, AMBER.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            "Not running as administrator: Stela may not be able to read or change the stick.",
                        )
                        .color(AMBER),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        #[cfg(windows)]
                        if ui.button("Restart as administrator").clicked()
                            && restart_as_administrator()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = ctx;
                            ui.label(RichText::new("Start it with sudo.").color(AMBER));
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }

    fn run_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, running: bool) {
        let erase = self.form.action == Action::Create && !self.form.dry_run;
        let create = self.form.action == Action::Create;
        let chosen = !self.form.stick.is_empty() && !self.form.confirm_disk.trim().is_empty();
        let ready = !running && (!create || chosen);
        let (text, fill) = if erase {
            ("Erase and create", RED)
        } else {
            ("Run", TEAL)
        };
        let button = egui::Button::new(RichText::new(text).strong().size(15.0).color(if erase {
            TEXT
        } else {
            BACKGROUND
        }))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(10))
        .min_size(vec2(150.0, 38.0));
        if ui.add_enabled(ready, button).clicked() {
            self.start(ctx);
        }
        if erase {
            ui.label(RichText::new("⚠ Everything on the stick is erased.").color(AMBER));
        }
        if create && !chosen && !running {
            ui.label(
                RichText::new("Choose the stick, then type its model or size to confirm.")
                    .color(MUTED),
            );
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let running = lock(&self.run).running;
        self.sidebar(ctx, running);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.heading(RichText::new(self.form.action.title()).strong());
                        ui.label(RichText::new(self.form.action.about()).color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        badge(ui, &lock(&self.run));
                    });
                });
                ui.add_space(8.0);
                self.rights_banner(ui, ctx);
                card(ui, |ui| {
                    ui.add_enabled_ui(!running, |ui| self.options(ui));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| self.run_button(ui, ctx, running));
                });
                ui.add_space(8.0);
                let run = lock(&self.run);
                let long = self.form.action.long();
                if long && run.action == Some(self.form.action) {
                    progress_card(ui, &run, self.step.as_ref());
                    ui.add_space(8.0);
                }
                let lines = run.lines.len();
                drop(run);
                if long {
                    egui::CollapsingHeader::new(
                        RichText::new(format!("Details · {lines} lines")).strong(),
                    )
                    .id_salt("details")
                    .default_open(false)
                    .show_unindented(ui, |ui| self.output(ui, running));
                } else {
                    ui.label(RichText::new("Output").strong());
                    self.output(ui, running);
                }
            });
        if self.was_running && !running {
            self.images = embed::cached_images();
        }
        self.was_running = running;
        if running {
            self.step = embed::progress().or(self.step.take());
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        #[cfg(windows)]
        {
            let state = {
                let run = lock(&self.run);
                match (run.running, run.succeeded) {
                    (true, _) => match fraction(self.step.as_ref()) {
                        Some(share) => taskbar::State::Share((share * 1000.0) as u16),
                        None => taskbar::State::Busy,
                    },
                    (false, Some(false)) => taskbar::State::Failed,
                    _ => taskbar::State::Off,
                }
            };
            if let Some(taskbar) = self
                .taskbar
                .get_or_insert_with(|| taskbar::Taskbar::of(frame))
            {
                taskbar.show(state);
            }
        }
        #[cfg(not(windows))]
        let _ = frame;
    }
}

impl App {
    /// Everything the command printed, with a button to clear it.
    fn output(&self, ui: &mut egui::Ui, running: bool) {
        egui::Frame::new()
            .fill(BACKGROUND)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let mut run = lock(&self.run);
                if ui
                    .add_enabled(!running, egui::Button::new("Clear").small())
                    .clicked()
                {
                    run.lines.clear();
                }
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if run.lines.is_empty() {
                            ui.label(
                                RichText::new("stela's output appears here.")
                                    .monospace()
                                    .color(MUTED),
                            );
                        }
                        for line in &run.lines {
                            ui.label(RichText::new(line).monospace().color(line_color(line)));
                        }
                    });
            });
    }
}

/// The share of its bytes `step` has done; a step without a size has none.
fn fraction(step: Option<&embed::Progress>) -> Option<f32> {
    let step = step?;
    let total = step.total.filter(|&total| total > 0)?;
    Some((step.done as f64 / total as f64).min(1.0) as f32)
}

/// `3.2 GB`, as stela writes sizes.
fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// `4 min 05 s`.
fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds / 60 {
        0 => format!("{seconds} s"),
        minutes => format!("{minutes} min {:02} s", seconds % 60),
    }
}

/// The run at a glance: the step it is at, a bar, and how long it took.
fn progress_card(ui: &mut egui::Ui, run: &Run, step: Option<&embed::Progress>) {
    let last = |error: bool| {
        run.lines
            .iter()
            .rev()
            .find(|line| !line.starts_with('$') && line.starts_with("error") == error)
            .cloned()
            .unwrap_or_default()
    };
    let failed = !run.running && run.succeeded == Some(false);
    let (title, color) = match (run.running, run.succeeded) {
        (true, _) => (
            step.map_or_else(
                || "Preparing…".to_owned(),
                |step| format!("{}…", step.label),
            ),
            TEAL,
        ),
        (false, Some(true)) => ("Finished".to_owned(), TEAL),
        _ => ("Stopped with an error".to_owned(), RED),
    };
    // The step's share of its bytes; a step without a size has none.
    let measured = step.and_then(|step| Some((step.done, step.total.filter(|&total| total > 0)?)));
    let fraction = match run.succeeded {
        Some(true) => Some(1.0),
        // Where it stopped, or a full red bar, as the taskbar shows it.
        Some(false) => fraction(step).or(Some(1.0)),
        None => fraction(step),
    };
    let detail = match measured {
        _ if failed => last(true),
        Some((done, total)) if run.running => {
            format!("{} of {}", gigabytes(done), gigabytes(total))
        }
        _ => last(false),
    };
    let elapsed = match (run.started, run.ended) {
        (Some(started), Some(ended)) => ended - started,
        (Some(started), None) => started.elapsed(),
        _ => Duration::ZERO,
    };
    card(ui, |ui| {
        ui.horizontal(|ui| {
            if run.running {
                ui.add(egui::Spinner::new().size(16.0).color(color));
            }
            ui.label(RichText::new(title).size(17.0).strong().color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(duration(elapsed)).color(MUTED));
            });
        });
        let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
            .desired_height(10.0)
            .corner_radius(CornerRadius::same(5))
            .fill(color);
        let bar = match fraction {
            Some(_) if run.running => bar.show_percentage(),
            Some(_) => bar,
            // No size to measure: the bar runs while the step does.
            None => bar.animate(run.running),
        };
        ui.add(bar);
        if !detail.is_empty() {
            ui.label(RichText::new(detail).small().color(MUTED));
        }
    });
}

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Stela")
        .with_inner_size([1040.0, 720.0])
        .with_min_inner_size([820.0, 560.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON) {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "Stela",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_spells_the_command_line() {
        let form = Form {
            action: Action::Create,
            stick: "/dev/sdb".into(),
            installer: "offline",
            wifi: " Home ".into(),
            confirm_disk: "64GB".into(),
            full: true,
            ..Form::default()
        };
        assert_eq!(
            form.args(),
            [
                "create",
                "--installer",
                "offline",
                "--wifi",
                "Home",
                "--confirm-disk",
                "64GB",
                "/dev/sdb",
                "--yes"
            ]
        );
        let form = Form {
            action: Action::Create,
            stick: "disk3".into(),
            confirm_disk: "31GB".into(),
            image_cache: "C:\\cache\\images".into(),
            ..Form::default()
        };
        assert_eq!(
            form.args(),
            [
                "create",
                "--confirm-disk",
                "31GB",
                "--download-dir",
                "C:\\cache\\images",
                "disk3",
                "--yes"
            ]
        );
        for form in [
            Form {
                installer: "online",
                ..form
            },
            Form {
                action: Action::Create,
                ipsw: "a.ipsw".into(),
                image_cache: "C:\\cache\\images".into(),
                ..Form::default()
            },
        ] {
            assert!(!form.args().iter().any(|arg| arg == "--download-dir"));
        }
        let form = Form {
            action: Action::Status,
            full: true,
            dry_run: true,
            ..Form::default()
        };
        assert_eq!(form.args(), ["status", "--full", "--dry-run", "--yes"]);
    }
}

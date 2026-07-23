//! Input mapping: SDL2 events (keyboard, Vita controller, touch) -> app-level commands.
//!
//! The Vita controller mapping GUID string and the front/rear touch device ids below are
//! adapted from `green-vita` (MPL-2.0, https://github.com/Day-OS/green-vita) - see
//! `THIRD_PARTY_NOTICES.md`. They encode non-obvious platform quirks (SDL's built-in Vita
//! controller driver does not ship a default `SDL_GameControllerDB` mapping, and VitaSDK's
//! `SDL_vitatouch.c` registers the front/back touch panels as SDL touch devices 1 and 2, in
//! that order) that are not documented anywhere else and are cheaper to keep than to
//! rediscover.

use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;

/// Directional/confirm navigation, independent of what screen is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCommand {
    Back,
    Confirm,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
}

/// Top-level command enum the shell feeds into `App::handle_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    Input(InputCommand),
    SetSearchQuery(String),
    /// Ask the shell to open the platform text-input method (SDL IME / on-screen keyboard).
    RequestSearch,
    /// Close/submit search input and stop the platform text-input method.
    CloseSearch,
    ToggleConfirmExit,
    CancelConfirmExit,
    ConfirmExitSession,
    /// Emitted by the catalog screen's language picker.
    SetLocale(crate::locale::Locale),
    /// Emitted by the catalog screen's sort picker.
    SetSort(crate::app::CatalogSort),
    /// Emitted when a row in the catalog list is tapped/clicked.
    SelectGame(usize),
}

impl From<InputCommand> for AppCommand {
    fn from(command: InputCommand) -> Self {
        AppCommand::Input(command)
    }
}

/// The front touchscreen's own SDL touch device id (registered second on the Vita's touch
/// backend, `SDL_vitatouch.c`: `SDL_AddTouch(1, ..., "Front")`, `SDL_AddTouch(2, ..., "Back")`).
const FRONT_TOUCH_DEVICE_ID: i64 = 1;

pub fn map_keyboard_event(event: &Event) -> Option<AppCommand> {
    let Event::KeyDown {
        keycode: Some(key),
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    let command = match *key {
        Keycode::Escape => InputCommand::Back,
        Keycode::Return => InputCommand::Confirm,
        Keycode::Up => InputCommand::MoveUp,
        Keycode::Down => InputCommand::MoveDown,
        Keycode::Left => InputCommand::MoveLeft,
        Keycode::Right => InputCommand::MoveRight,
        _ => return None,
    };
    Some(command.into())
}

pub fn map_controller_button_event(event: &Event) -> Option<AppCommand> {
    match event {
        Event::ControllerButtonDown {
            button: Button::B, ..
        } => Some(InputCommand::Back.into()),
        Event::ControllerButtonDown {
            button: Button::A, ..
        } => Some(InputCommand::Confirm.into()),
        _ => None,
    }
}

const MENU_STICK_DEADZONE: f32 = 0.5;

/// Polled every frame (rather than event-driven) so holding a direction auto-repeats; see the
/// repeat-delay/interval constants in `shell::run`.
pub fn held_menu_direction(controller: Option<&GameController>) -> Option<InputCommand> {
    let controller = controller?;
    if controller.button(Button::DPadUp) {
        return Some(InputCommand::MoveUp);
    }
    if controller.button(Button::DPadDown) {
        return Some(InputCommand::MoveDown);
    }
    if controller.button(Button::DPadLeft) {
        return Some(InputCommand::MoveLeft);
    }
    if controller.button(Button::DPadRight) {
        return Some(InputCommand::MoveRight);
    }
    let x = axis_to_f32(controller.axis(Axis::LeftX));
    let y = axis_to_f32(controller.axis(Axis::LeftY));
    if y.abs() >= x.abs() {
        match y {
            y if y <= -MENU_STICK_DEADZONE => Some(InputCommand::MoveUp),
            y if y >= MENU_STICK_DEADZONE => Some(InputCommand::MoveDown),
            _ => None,
        }
    } else {
        match x {
            x if x <= -MENU_STICK_DEADZONE => Some(InputCommand::MoveLeft),
            x if x >= MENU_STICK_DEADZONE => Some(InputCommand::MoveRight),
            _ => None,
        }
    }
}

fn axis_to_f32(raw: i16) -> f32 {
    raw as f32 / i16::MAX as f32
}

/// SDL's built-in Vita controller driver reports raw joystick buttons/axes but does not ship a
/// `SDL_GameControllerDB` mapping for the Vita's own front controls, so `GameControllerSubsystem`
/// would otherwise never recognize it as a "game controller".
pub fn register_vita_controller_mapping(sdl: &sdl2::Sdl) -> Result<(), String> {
    let joystick_subsystem = sdl.joystick()?;
    if joystick_subsystem
        .num_joysticks()
        .map_err(|e| e.to_string())?
        == 0
    {
        return Ok(());
    }
    let guid = joystick_subsystem
        .device_guid(0)
        .map_err(|e| e.to_string())?;
    let mapping = format!(
        "{guid},PSVita Controller,\
         a:b2,b:b1,x:b3,y:b0,\
         back:b10,start:b11,\
         leftshoulder:b4,rightshoulder:b5,\
         leftstick:b14,rightstick:b15,\
         dpup:b8,dpdown:b6,dpleft:b7,dpright:b9,\
         leftx:a0,lefty:a1,rightx:a2,righty:a3,\
         lefttrigger:b12,righttrigger:b13,platform:PS Vita,"
    );
    sdl.game_controller()?
        .add_mapping(&mapping)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open_first_controller(subsystem: &sdl2::GameControllerSubsystem) -> Option<GameController> {
    let available = subsystem.num_joysticks().ok()?;
    (0..available).find_map(|id| {
        if !subsystem.is_game_controller(id) {
            return None;
        }
        subsystem.open(id).ok()
    })
}

/// Forwards mouse/front-touch input to egui.
pub fn map_pointer_event(
    event: &Event,
    screen_size: (f32, f32),
    pixels_per_point: f32,
    pointer_pos: &mut egui::Pos2,
) -> Option<egui::Event> {
    match *event {
        Event::MouseMotion { x, y, .. } => {
            *pointer_pos = mouse_to_screen_pos(x, y, pixels_per_point);
            Some(egui::Event::PointerMoved(*pointer_pos))
        }
        Event::MouseButtonDown {
            mouse_btn, x, y, ..
        } => Some(pointer_button_at(
            pointer_pos,
            mouse_to_screen_pos(x, y, pixels_per_point),
            map_mouse_button(mouse_btn),
            true,
        )),
        Event::MouseButtonUp {
            mouse_btn, x, y, ..
        } => Some(pointer_button_at(
            pointer_pos,
            mouse_to_screen_pos(x, y, pixels_per_point),
            map_mouse_button(mouse_btn),
            false,
        )),
        Event::FingerDown { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            Some(pointer_button_at(
                pointer_pos,
                touch_to_screen_pos(x, y, screen_size),
                egui::PointerButton::Primary,
                true,
            ))
        }
        Event::FingerMotion { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            *pointer_pos = touch_to_screen_pos(x, y, screen_size);
            Some(egui::Event::PointerMoved(*pointer_pos))
        }
        Event::FingerUp { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            Some(pointer_button_at(
                pointer_pos,
                touch_to_screen_pos(x, y, screen_size),
                egui::PointerButton::Primary,
                false,
            ))
        }
        _ => None,
    }
}

fn pointer_button_at(
    pointer_pos: &mut egui::Pos2,
    pos: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
) -> egui::Event {
    *pointer_pos = pos;
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::default(),
    }
}

fn mouse_to_screen_pos(x: i32, y: i32, pixels_per_point: f32) -> egui::Pos2 {
    egui::pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point)
}

fn touch_to_screen_pos(x: f32, y: f32, (width, height): (f32, f32)) -> egui::Pos2 {
    egui::pos2(x * width, y * height)
}

fn map_mouse_button(button: MouseButton) -> egui::PointerButton {
    match button {
        MouseButton::Right => egui::PointerButton::Secondary,
        MouseButton::Middle => egui::PointerButton::Middle,
        _ => egui::PointerButton::Primary,
    }
}

/// Full controller snapshot for the streaming session, in XInput conventions (the format the NVST
/// input channel speaks - see `gfn::input_protocol`).
pub fn gamepad_snapshot(controller: &GameController) -> crate::gfn::input_protocol::GamepadInput {
    const DPAD_UP: u16 = 0x0001;
    const DPAD_DOWN: u16 = 0x0002;
    const DPAD_LEFT: u16 = 0x0004;
    const DPAD_RIGHT: u16 = 0x0008;
    const START: u16 = 0x0010;
    const BACK: u16 = 0x0020;
    const LEFT_THUMB: u16 = 0x0040;
    const RIGHT_THUMB: u16 = 0x0080;
    const LEFT_SHOULDER: u16 = 0x0100;
    const RIGHT_SHOULDER: u16 = 0x0200;
    const A: u16 = 0x1000;
    const B: u16 = 0x2000;
    const X: u16 = 0x4000;
    const Y: u16 = 0x8000;

    let mut buttons = 0u16;
    let mut set = |button: Button, mask: u16| {
        if controller.button(button) {
            buttons |= mask;
        }
    };
    set(Button::DPadUp, DPAD_UP);
    set(Button::DPadDown, DPAD_DOWN);
    set(Button::DPadLeft, DPAD_LEFT);
    set(Button::DPadRight, DPAD_RIGHT);
    set(Button::Start, START);
    set(Button::Back, BACK);
    set(Button::LeftStick, LEFT_THUMB);
    set(Button::RightStick, RIGHT_THUMB);
    set(Button::LeftShoulder, LEFT_SHOULDER);
    set(Button::RightShoulder, RIGHT_SHOULDER);
    set(Button::A, A);
    set(Button::B, B);
    set(Button::X, X);
    set(Button::Y, Y);

    let axis = |axis: Axis| controller.axis(axis);
    let trigger = |value: i16| (value.max(0) / 129).min(255) as u8;

    crate::gfn::input_protocol::GamepadInput {
        controller_id: 0,
        buttons,
        left_trigger: trigger(axis(Axis::TriggerLeft)),
        right_trigger: trigger(axis(Axis::TriggerRight)),
        left_stick_x: axis(Axis::LeftX),
        left_stick_y: axis(Axis::LeftY).saturating_neg(),
        right_stick_x: axis(Axis::RightX),
        right_stick_y: axis(Axis::RightY).saturating_neg(),
        timestamp_us: 0,
    }
}

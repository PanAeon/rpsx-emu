use std::{any::Any, collections::VecDeque, fs::File};

use arrayvec::ArrayVec;
use gilrs::{
    GamepadId, Gilrs,
    ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks},
};
use log::info;
use serde::{Deserialize, Serialize};

use crate::system::{Addressable, System};

#[derive(Serialize, Deserialize)]
pub struct Sio {
    control: Control,
    boudrate_reload: u16,
    mode: Mode,
    status: Status,
    transfer: Option<u8>,
    received: ArrayVec<u8, 8>,

    // move out?
    state: State,
    pub gamepad1: Box<dyn Controller>,
    pub gamepad2: Box<dyn Controller>,
    pub memcard1: Memcard,
    pub memcard2: Memcard,
}

impl Sio {
    pub fn new(negcon: bool, mouse1: bool, mouse2: bool, guncon: bool, justifier: bool) -> Self {
        let gamepad1: Box<dyn Controller> = if justifier {
            Box::new(Justifier::default())
        } else if guncon {
            Box::new(GunCon::default())
        } else if mouse1 {
            Box::new(Mouse::default())
        } else if negcon {
            Box::new(NegCon::default())
        } else {
            // Box::new(DualShock::initial())
            Box::new(Dualshock::default())
        };
        let gamepad2: Box<dyn Controller> = if mouse2 {
            Box::new(Mouse::default())
        } else {
            Box::new(Dualshock::default())
        };
        Sio {
            control: Control::default(),
            boudrate_reload: 0,
            mode: Mode::default(),
            status: Status::default(), // TX idle and TX ready
            transfer: None,
            received: ArrayVec::new(),
            state: State::None,
            gamepad1,
            gamepad2,
            memcard1: Memcard::new("/foo/memcard1.mcd"),
            memcard2: Memcard::new("/foo/memcard2.mcd"),
        }
    }

    pub fn load<T: Addressable>(&mut self, offset: u32) -> T {
        match offset {
            0x0 => T::from_u32(self.pop_received_data()),
            0x4 => {
                // self.status.set_tx_fifo_not_full(true);
                // self.status.set_tx_idle(true);
                // self.status.set_rx_fifo_not_empty(true);
                T::from_u32(self.status.0)
            }
            0xA => T::from_u32(self.control.0 as u32),
            0xE => T::from_u32(self.boudrate_reload as u32), // baud rate
            _ => panic!("Unhandled sio load{:?} offset: {:08x}", T::width(), offset),
        }
    }

    pub fn store<T: Addressable>(system: &mut System, offset: u32, value: T) {
        let sio = &mut system.sio;
        // 1F801040h+N*10h - SIO#_TX_DATA (W)
        let val = value.as_u32() as u16;
        match offset {
            0x0 => {
                sio.transfer = Some(val as u8);
                Self::try_send_data(system);
                // try send data
            }
            0x8 => {
                sio.mode.0 = val & 0x1FF;
            }
            0xA => Self::write_control(system, val),
            0xE => {
                sio.boudrate_reload = val;
            }
            _ => panic!("Unhandled sio store{:?} offset: {:08x}", T::width(), offset),
        };
    }

    pub fn write_control(system: &mut System, val: u16) {
        let sio = &mut system.sio;
        sio.control.0 = val & !0xC000;
        // println!("control: {:?}", self.control);

        if sio.control.acknowledge() {
            sio.status.set_interrupt_request(false);
            sio.control.set_acknowledge(false);
        }

        if sio.control.reset() {
            sio.reset_regs();
        }

        if !sio.control.dtr_output_level() {
            sio.gamepad1.reset();
            sio.gamepad2.reset();
            sio.memcard1.reset();
            sio.memcard2.reset();
            sio.status.set_dsr_input_level(false);
        }

        if sio.control.tx_enable() {
            Self::try_send_data(system);
            // try send data..
        }
    }

    pub fn reset_regs(&mut self) {
        self.transfer = None;
        self.received = ArrayVec::default();
        self.status = Status(0x22005);
        self.control = Control::default();
        self.mode.0 = 0;
        self.boudrate_reload = 0;
        self.state = State::None;
        self.gamepad1.reset();
        self.gamepad2.reset();
        self.memcard1.reset();
        self.memcard2.reset();
    }

    pub fn pop_received_data(&mut self) -> u32 {
        let data = self.received.pop_at(0).unwrap_or(0xFF);
        if self.received.is_empty() {
            self.status.set_rx_fifo_not_empty(false);
        }
        data.into()
    }

    pub fn push_received_data(&mut self, msg: u8) {
        if !self.control.rx_enable() && !self.control.dtr_output_level() {
            return;
        }

        if self.received.is_full() {
            *self.received.last_mut().expect("last received") = msg;
        } else {
            self.received.push(msg);
        }
        self.status.set_rx_fifo_not_empty(true);
        self.control.set_rx_enabled(false);
    }

    pub fn send_and_receive_byte(&mut self, gilrs: &mut Gilrs, data: u8) -> (u8, bool) {
        let port = usize::from(self.control.port_select());

        let (byte, next_state) = match self.state {
            State::None => match data {
                0x01 => self.process_gamepad(gilrs, port, data),
                0x81 => self.process_memcard(port, data),
                _ => (0xFF, State::None),
            },
            State::GamepadComm => self.process_gamepad(gilrs, port, data),
            State::MemcardComm => self.process_memcard(port, data),
        };
        self.state = next_state;
        (byte, next_state != State::None)
    }

    // let's start with one gamepad and one memcard for now...
    pub fn process_gamepad(&mut self, gilrs: &mut Gilrs, port: usize, data: u8) -> (u8, State) {
        if port == 0 {
            let byte = self.gamepad1.send_and_receive_byte(gilrs, data);
            let state = if self.gamepad1.in_ack() {
                State::GamepadComm
            } else {
                State::None
            };
            (byte, state)
        } else {
            let byte = self.gamepad2.send_and_receive_byte(gilrs, data);
            let state = if self.gamepad2.in_ack() {
                State::GamepadComm
            } else {
                State::None
            };
            (byte, state)
            // (0xFF, State::None)
        }
        // (0xFF, State::None)
    }
    pub fn process_memcard(&mut self, port: usize, data: u8) -> (u8, State) {
        if port == 0 {
            let byte = self.memcard1.send_and_receive_byte(data);
            let state = if self.memcard1.in_ack() {
                State::MemcardComm
            } else {
                State::None
            };
            (byte, state)
        } else {
            let byte = self.memcard2.send_and_receive_byte(data);
            let state = if self.memcard2.in_ack() {
                State::MemcardComm
            } else {
                State::None
            };
            (byte, state)
            // (0xFF, State::None)
        }
        // (0xFF, State::None)
    }

    pub fn try_send_data(system: &mut System) {
        if !system.sio.control.tx_enable() {
            return;
        }

        if let Some(val) = system.sio.transfer.take() {
            // send/receive

            let gilrs = system.gilrs.as_mut().unwrap();
            let (received, ack) = system.sio.send_and_receive_byte(gilrs, val);

            let sio = &mut system.sio;
            sio.status.set_dsr_input_level(ack);

            if sio.control.dsr_interrupt_enable() && sio.status.dsr_input_level() {
                system.scheduler.schedule(
                    crate::scheduler::Event::SerialSend,
                    u64::from(sio.boudrate_reload) * 8,
                    None,
                );
            }

            if sio.status.dsr_input_level() {
                system
                    .scheduler
                    .schedule(crate::scheduler::Event::DsrOff, 96, None);
            }

            system.sio.push_received_data(received);
        }
    }

    pub fn turn_dsr_off(&mut self) {
        self.status.set_dsr_input_level(false);
    }

    pub fn process_serial_send(system: &mut System) {
        system.irqctl.set_ctl_mem();
        system.sio.status.set_interrupt_request(true);
    }
}

// 0     TX Enable (TXEN)      (0=Disable, 1=Enable)
// 1     DTR Output Level      (0=Off, 1=On)
// 2     RX Enable (RXEN)      (SIO1: 0=Disable, 1=Enable)  ;Disable also clears RXFIFO
//                             (SIO0: 0=only receive when /CS low, 1=force receiving single byte)
// 3     SIO1 TX Output Level  (0=Normal, 1=Inverted, during Inactivity & Stop bits)
// 4     Acknowledge           (0=No change, 1=Reset SIO_STAT.Bits 3,4,5,9)      (W)
// 5     SIO1 RTS Output Level (0=Off, 1=On)
// 6     Reset                 (0=No change, 1=Reset most registers to zero) (W)
// 7     SIO1 unknown?         (read/write-able when FACTOR non-zero) (otherwise always zero)
// 8-9   RX Interrupt Mode     (0..3 = IRQ when RX FIFO contains 1,2,4,8 bytes)
// 10    TX Interrupt Enable   (0=Disable, 1=Enable) ;when SIO_STAT.0-or-2 ;Ready
// 11    RX Interrupt Enable   (0=Disable, 1=Enable) ;when N bytes in RX FIFO
// 12    DSR Interrupt Enable  (0=Disable, 1=Enable) ;when SIO_STAT.7  ;DSR high or /ACK low
// 13    SIO0 port select      (0=port 1, 1=port 2) (/CS pulled low when bit 1 set)
// 14-15 Not used              (always zero)

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct Control(u16);
    impl Debug;
    tx_enable, _: 0;
    dtr_output_level, _: 1;
    rx_enable, set_rx_enabled: 2;
    tx_output_level, _: 3;
    acknowledge, set_acknowledge: 4;
    rts_output_level, _: 5;
    reset, _: 6;
    rx_interrupt_mode, _: 9,8;
    tx_interrupt_enable, _: 10;
    rx_interrupt_enable, _: 11;
    dsr_interrupt_enable, _: 12;
    port_select, _: 13;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct Mode(u16);
    impl Debug;
    boudrate_reload_factor, _: 1,0;
    character_length, _: 3,2;
    parity_enable, _: 4;
    parity_type, _:5;
    stop_bit_length, _: 7,6;
    clock_polarity, _: 8;
}

// 0     TX FIFO Not Full       (1=Ready for new byte)  (depends on CTS) (TX requires CTS)
// 1     RX FIFO Not Empty      (0=Empty, 1=Data available)
// 2     TX Idle                (1=Idle/Finished)       (depends on TXEN and on CTS)
// 3     RX Parity Error        (0=No, 1=Error; Wrong Parity, when enabled) (sticky)
// 4     SIO1 RX FIFO Overrun   (0=No, 1=Error; received more than 8 bytes) (sticky)
// 5     SIO1 RX Bad Stop Bit   (0=No, 1=Error; Bad Stop Bit) (when RXEN)   (sticky)
// 6     SIO1 RX Input Level    (0=Normal, 1=Inverted) ;only AFTER receiving Stop Bit
// 7     DSR Input Level        (0=Off, 1=On) (remote DTR) ;DSR not required to be on
// 8     SIO1 CTS Input Level   (0=Off, 1=On) (remote RTS) ;CTS required for TX
// 9     Interrupt Request      (0=None, 1=IRQ) (See SIO_CTRL.Bit4,10-12)   (sticky)
// 10    Unknown                (always zero)
// 11-31 Baudrate Timer         (15-21 bit timer, decrementing at 33MHz)
bitfield::bitfield! {
    #[derive(Serialize, Deserialize)]
    pub struct Status(u32);
    impl Debug;
    tx_fifo_not_full, set_tx_fifo_not_full: 0;
    rx_fifo_not_empty, set_rx_fifo_not_empty: 1;
    tx_idle, set_tx_idle: 2;
    rx_parity_error, set_rx_parity_error: 3;
    rx_fifo_overrun, _: 4;
    rx_bad_stop_bit, _: 5;
    rx_input_level, _: 6;
    dsr_input_level, set_dsr_input_level: 7;
    cts_input_level, set_cts_input_levvel: 8;
    interrupt_request, set_interrupt_request: 9;
    baudrate_timer, set_baudrate_timer: 31,11;

}

impl Default for Status {
    fn default() -> Self {
        Self(0x22005) // TX idle and TX ready
    }
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum State {
    None,
    GamepadComm,
    MemcardComm,
}

#[typetag::serde(tag = "type")]
pub trait Controller {
    fn send_and_receive_byte(&mut self, gilrs: &mut Gilrs, data: u8) -> u8;
    fn in_ack(&self) -> bool;

    fn reset(&mut self);

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8));

    fn get_stick_axis(&mut self) -> &mut [u8; 4];

    fn set_buttons(&mut self, buttons: u16);

    fn get_buttons(&self) -> u16;

    fn set_analog_mode(&mut self, is_analog: bool);
    fn get_analog_mode(&self) -> bool;

    fn as_any(&mut self) -> &mut dyn Any;
}

#[derive(Serialize, Deserialize)]
pub struct NegCon {
    state: NegConState,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],
}
impl Default for NegCon {
    fn default() -> Self {
        Self {
            state: NegConState::default(),
            digital_switches: 0xFFFF,
            joystick_axes: [0x80; 4],
            in_ack: Default::default(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum NegConState {
    #[default]
    Init,
    IdLow,
    IdHigh,
    SwitchLow,
    SwitchHigh,
    AnalogInput0,
    AnalogInput1,
    AnalogInput2,
    AnalogInput3,
}

#[typetag::serde]
impl Controller for NegCon {
    fn send_and_receive_byte(&mut self, _: &mut Gilrs, data: u8) -> u8 {
        const ID: [u8; 2] = 0x5A23_u16.to_le_bytes();
        let received = match self.state {
            NegConState::Init => 0xFF,

            NegConState::IdLow => ID[0],
            NegConState::IdHigh => ID[1],

            NegConState::SwitchLow => self.digital_switches as u8,
            NegConState::SwitchHigh => (self.digital_switches >> 8) as u8,

            NegConState::AnalogInput0 => self.joystick_axes[Axis::RightX as usize],
            NegConState::AnalogInput1 => self.joystick_axes[Axis::RightY as usize],
            NegConState::AnalogInput2 => self.joystick_axes[Axis::LeftX as usize],
            NegConState::AnalogInput3 => self.joystick_axes[Axis::LeftY as usize],
        };

        // if self.state == NegConState::SwitchLow {
        //     println!("buttons: 0x{:X}", self.digital_switches);
        // }

        if let Some(state) = self.next(self.state, data) {
            self.state = state;
            self.in_ack = state != NegConState::Init;
            received
        } else {
            self.reset();
            0xFF
        }
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        self.in_ack = false;
        self.state = NegConState::Init;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }
    fn set_analog_mode(&mut self, _is_analog: bool) {
        info!("analog mode is ignored for negcon")
    }
    fn get_analog_mode(&self) -> bool {
        true
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl NegCon {
    const ANALOG_STATES: [(NegConState, Option<u8>); 9] = [
        (NegConState::Init, Some(0x00)),
        (NegConState::IdLow, Some(0x01)),
        (NegConState::IdHigh, Some(0x42)),
        (NegConState::SwitchLow, None),
        (NegConState::SwitchHigh, None),
        (NegConState::AnalogInput0, None),
        (NegConState::AnalogInput1, Some(0x00)),
        (NegConState::AnalogInput2, Some(0x00)),
        (NegConState::AnalogInput3, Some(0x00)),
    ];
    fn next(&self, current_state: NegConState, received_byte: u8) -> Option<NegConState> {
        let idx = current_state as usize;

        let (next_state, check_byte) = Self::ANALOG_STATES[(idx + 1) % Self::ANALOG_STATES.len()];
        // if current_state == GamepadState::IdLow {
        // println!("data: {:X}, next_state: {:?}, check_byte: {:?}", received_byte, next_state, check_byte);
        // }
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Mouse {
    state: MouseState,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],
}
impl Default for Mouse {
    fn default() -> Self {
        Self {
            state: MouseState::default(),
            digital_switches: 0xFFFF,
            joystick_axes: [0x80; 4],
            in_ack: Default::default(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum MouseState {
    #[default]
    Init,
    IdLow,
    IdHigh,
    SwitchLow,
    SwitchHigh,
    AnalogInput0,
    AnalogInput1,
    // AnalogInput2,
    // AnalogInput3,
}

#[typetag::serde]
impl Controller for Mouse {
    fn send_and_receive_byte(&mut self, _: &mut Gilrs, data: u8) -> u8 {
        const ID: [u8; 2] = 0x5A12_u16.to_le_bytes();
        let received = match self.state {
            MouseState::Init => 0xFF,

            MouseState::IdLow => ID[0],
            MouseState::IdHigh => ID[1],

            MouseState::SwitchLow => self.digital_switches as u8, // not used
            MouseState::SwitchHigh => (self.digital_switches >> 8) as u8, // 10 right, 11 left

            MouseState::AnalogInput0 => self.joystick_axes[Axis::RightX as usize],
            MouseState::AnalogInput1 => self.joystick_axes[Axis::RightY as usize],
            // MouseState::AnalogInput2 => self.joystick_axes[Axis::LeftX as usize],
            // MouseState::AnalogInput3 => self.joystick_axes[Axis::LeftY as usize],
        };

        // if self.state == NegConState::SwitchLow {
        //     println!("buttons: 0x{:X}", self.digital_switches);
        // }

        if let Some(state) = self.next(self.state, data) {
            self.state = state;
            self.in_ack = state != MouseState::Init;
            received
        } else {
            self.reset();
            0xFF
        }
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        self.in_ack = false;
        self.state = MouseState::Init;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }
    fn set_analog_mode(&mut self, _is_analog: bool) {
        info!("analog mode is ignored for negcon")
    }
    fn get_analog_mode(&self) -> bool {
        true
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl Mouse {
    const ANALOG_STATES: [(MouseState, Option<u8>); 7] = [
        (MouseState::Init, Some(0x00)),
        (MouseState::IdLow, Some(0x01)),
        (MouseState::IdHigh, Some(0x42)),
        (MouseState::SwitchLow, None),
        (MouseState::SwitchHigh, None),
        (MouseState::AnalogInput0, None),
        (MouseState::AnalogInput1, Some(0x00)),
        // (MouseState::AnalogInput2, Some(0x00)),
        // (MouseState::AnalogInput3, Some(0x00)),
    ];
    fn next(&self, current_state: MouseState, received_byte: u8) -> Option<MouseState> {
        let idx = current_state as usize;

        let (next_state, check_byte) = Self::ANALOG_STATES[(idx + 1) % Self::ANALOG_STATES.len()];
        // if current_state == GamepadState::IdLow {
        // println!("data: {:X}, next_state: {:?}, check_byte: {:?}", received_byte, next_state, check_byte);
        // }
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

#[derive(Serialize, Deserialize)]
pub struct GunCon {
    state: GunConState,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],
    pub shoot_offscreen: bool,
}
impl Default for GunCon {
    fn default() -> Self {
        Self {
            state: GunConState::default(),
            digital_switches: 0xFFFF,
            joystick_axes: [0x0d, 0x01, 0x88, 0x00],
            in_ack: Default::default(),
            shoot_offscreen: false,
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum GunConState {
    #[default]
    Init,
    IdLow,
    IdHigh,
    SwitchLow,
    SwitchHigh,
    AnalogInput0,
    AnalogInput1,
    AnalogInput2,
    AnalogInput3,
}

/*
*  __Halfword 0 (Controller Info)___________________
  0-15  Controller Info  (5A63h=Namco Lightgun; GunCon/Cinch Type)
  __Halfword 1 (Buttons)___________________________
  0-2   Not used              (All bits always 1)
  3     Button A (Left Side)  (0=Pressed, 1=Released) ;aka Joypad Start
  4-12  Not used              (All bits always 1)
  13    Trigger Button        (0=Pressed, 1=Released) ;aka Joypad O-Button
  14    Button B (Right Side) (0=Pressed, 1=Released) ;aka Joypad X-Button
  15    Not used              (All bits always 1)
  __Halfword 2 (X)_________________________________
  0-15  8MHz clks since HSYNC (01h=Error, or 04Dh..1CDh)
  __Halfword 3 (Y)_________________________________
  0-15  Scanlines since VSYNC (05h/0Ah=Error, PAL=20h..127h, NTSC=19h..F8h)
*/
#[typetag::serde]
impl Controller for GunCon {
    fn send_and_receive_byte(&mut self, _: &mut Gilrs, data: u8) -> u8 {
        const ID: [u8; 2] = 0x5A63_u16.to_le_bytes();
        let axes = if self.shoot_offscreen {
            &[0x01, 0x00, 0xa, 0x00]
        } else {
            &self.joystick_axes
        };
        let received = match self.state {
            GunConState::Init => 0xFF,

            GunConState::IdLow => ID[0],
            GunConState::IdHigh => ID[1],

            GunConState::SwitchLow => self.digital_switches as u8, // not used
            GunConState::SwitchHigh => (self.digital_switches >> 8) as u8, // 10 right, 11 left

            GunConState::AnalogInput0 => axes[Axis::RightX as usize],
            GunConState::AnalogInput1 => axes[Axis::RightY as usize],
            GunConState::AnalogInput2 => axes[Axis::LeftX as usize],
            GunConState::AnalogInput3 => axes[Axis::LeftY as usize],
        };

        if let Some(state) = self.next(self.state, data) {
            self.state = state;
            self.in_ack = state != GunConState::Init;
            received
        } else {
            self.reset();
            0xFF
        }
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        self.in_ack = false;
        self.state = GunConState::Init;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }
    fn set_analog_mode(&mut self, _is_analog: bool) {
        info!("analog mode is ignored for negcon")
    }
    fn get_analog_mode(&self) -> bool {
        true
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl GunCon {
    const ANALOG_STATES: [(GunConState, Option<u8>); 9] = [
        (GunConState::Init, Some(0x00)),
        (GunConState::IdLow, Some(0x01)),
        (GunConState::IdHigh, Some(0x42)),
        (GunConState::SwitchLow, None),
        (GunConState::SwitchHigh, None),
        (GunConState::AnalogInput0, None),
        (GunConState::AnalogInput1, Some(0x00)),
        (GunConState::AnalogInput2, Some(0x00)),
        (GunConState::AnalogInput3, Some(0x00)),
    ];
    fn next(&self, current_state: GunConState, received_byte: u8) -> Option<GunConState> {
        let idx = current_state as usize;

        let (next_state, check_byte) = Self::ANALOG_STATES[(idx + 1) % Self::ANALOG_STATES.len()];
        // if current_state == GamepadState::IdLow {
        // println!("data: {:X}, next_state: {:?}, check_byte: {:?}", received_byte, next_state, check_byte);
        // }
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Justifier {
    state: JustifierState,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],
    pub shoot_offscreen: bool,
}
impl Default for Justifier {
    fn default() -> Self {
        Self {
            state: JustifierState::default(),
            digital_switches: 0xFFFF,
            joystick_axes: [0x0, 0x0, 0x0, 0x0],
            in_ack: Default::default(),
            shoot_offscreen: false,
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum JustifierState {
    #[default]
    Init,
    IdLow,
    IdHigh,
    SwitchLow,
    SwitchHigh,
    // AnalogInput0,
    // AnalogInput1,
    // AnalogInput2,
    // AnalogInput3,
}

/*
* __Halfword 0 (Controller Info)___________________
  0-15  Controller Info  (5A31h=Konami Lightgun; Timer/IRQ10 type)
  __Halfword 1 (Buttons)
  0-2   Not used                 (All bits always 1)
  3     Start Button (Left Side) (0=Pressed, 1=Released)  ;aka Joypad Start
  4-13  Not used                 (All bits always 1)
  14    Back Button  (Rear End)  (0=Pressed, 1=Released)  ;aka Joypad X-Button
  15    Trigger Button           (0=Pressed, 1=Released)  ;aka Joypad []-Button
*/

#[typetag::serde]
impl Controller for Justifier {
    fn send_and_receive_byte(&mut self, _: &mut Gilrs, data: u8) -> u8 {
        const ID: [u8; 2] = 0x5A31_u16.to_le_bytes();
        let received = match self.state {
            JustifierState::Init => 0xFF,

            JustifierState::IdLow => ID[0],
            JustifierState::IdHigh => ID[1],

            JustifierState::SwitchLow => self.digital_switches as u8,
            JustifierState::SwitchHigh => (self.digital_switches >> 8) as u8,
            // JustifierState::AnalogInput0 => 0,
            // JustifierState::AnalogInput1 => 0,
            // JustifierState::AnalogInput2 => axes[Axis::LeftX as usize],
            // JustifierState::AnalogInput3 => axes[Axis::LeftY as usize],
        };

        if let Some(state) = self.next(self.state, data) {
            self.state = state;
            self.in_ack = state != JustifierState::Init;
            received
        } else {
            self.reset();
            0xFF
        }
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        self.in_ack = false;
        self.state = JustifierState::Init;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }
    fn set_analog_mode(&mut self, _is_analog: bool) {
        info!("analog mode is ignored for negcon")
    }
    fn get_analog_mode(&self) -> bool {
        true
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl Justifier {
    const ANALOG_STATES: [(JustifierState, Option<u8>); 5] = [
        (JustifierState::Init, Some(0x00)),
        (JustifierState::IdLow, Some(0x01)),
        (JustifierState::IdHigh, Some(0x42)),
        (JustifierState::SwitchLow, None),
        (JustifierState::SwitchHigh, None),
        // (JustifierState::AnalogInput0, None),
        // (JustifierState::AnalogInput1, None),
    ];
    fn next(&self, current_state: JustifierState, received_byte: u8) -> Option<JustifierState> {
        let idx = current_state as usize;

        let (next_state, check_byte) = Self::ANALOG_STATES[(idx + 1) % Self::ANALOG_STATES.len()];
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Gamepad {
    state: GamepadState,
    pub mode: GamepadMode,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],
}

#[typetag::serde]
impl Controller for Gamepad {
    fn send_and_receive_byte(&mut self, _: &mut Gilrs, data: u8) -> u8 {
        let received = match self.state {
            GamepadState::Init => 0xFF,

            GamepadState::IdLow => self.mode.id()[0],
            GamepadState::IdHigh => self.mode.id()[1],

            GamepadState::SwitchLow => self.digital_switches as u8,
            GamepadState::SwitchHigh => (self.digital_switches >> 8) as u8,

            GamepadState::AnalogInput0 => self.joystick_axes[Axis::RightX as usize],
            GamepadState::AnalogInput1 => self.joystick_axes[Axis::RightY as usize],
            GamepadState::AnalogInput2 => self.joystick_axes[Axis::LeftX as usize],
            GamepadState::AnalogInput3 => self.joystick_axes[Axis::LeftY as usize],
        };

        // if self.state == GamepadState::SwitchLow {
        //     println!("buttons: 0x{:X}", self.digital_switches);
        // }

        if let Some(state) = self.mode.next(self.state, data) {
            self.state = state;
            self.in_ack = state != GamepadState::Init;
            received
        } else {
            self.reset();
            0xFF
        }
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        // println!("reset");
        self.in_ack = false;
        self.state = GamepadState::Init;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }

    fn set_analog_mode(&mut self, is_analog: bool) {
        if self.mode.get() == is_analog {
            return;
        }
        self.mode.set(is_analog);
        self.reset();
    }
    fn get_analog_mode(&self) -> bool {
        self.mode.get()
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for Gamepad {
    fn default() -> Self {
        Self {
            state: GamepadState::default(),
            mode: GamepadMode::default(),
            digital_switches: 0xFFFF,
            joystick_axes: [0x80; 4],
            in_ack: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum DualshockCommand {
    Idle,
    Ready,
    ReadPad,
    ConfigModeSetMode,
    SetAnalogueMode,
    GetAnalogMode,
    Command46,
    Command47,
    Command4C,
    GetSetRumble,
}

#[derive(Serialize, Deserialize)]
pub struct Dualshock {
    cmd: DualshockCommand,
    tx_buffer: VecDeque<u8>,
    rx_buffer: Vec<u8>,
    cmd_step: u32,
    configuration_mode: bool,
    analog_mode: bool,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8; 4],

    pub rumble_config: [u8; 6],
    motor_state: [u8; 2],
    vibration_bias: [i16; 2],
    // pub small_motor_strength: f32
    #[serde(skip)]
    ff_effect: Option<gilrs::ff::Effect>,

    #[serde(skip)]
    active_gamepad: Option<GamepadId>,
    analog_locked: bool,
}

impl Dualshock {
    fn get_mode_id(&self) -> u8 {
        if self.configuration_mode {
            0xF
        } else if self.analog_mode {
            0x7
        } else {
            0x4
        }
    }

    fn get_response_num_halfword(&self) -> u8 {
        if self.configuration_mode || self.analog_mode {
            0x3
        } else {
            0x1
        }
    }

    fn get_id_byte(&self) -> u8 {
        self.get_mode_id() << 4 | self.get_response_num_halfword()
    }

    fn poll(&mut self) {
        let status_byte = 0x5A; // could be 0x0
        // tx_buffer = {GetIDByte(), m_status_byte, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00};
        self.tx_buffer.clear();
        self.tx_buffer.push_back(self.get_id_byte());
        self.tx_buffer.push_back(status_byte);

        self.tx_buffer.push_back(self.digital_switches as u8);
        self.tx_buffer.push_back((self.digital_switches >> 8) as u8);

        if self.analog_mode || self.configuration_mode {
            self.tx_buffer
                .push_back(self.joystick_axes[Axis::RightX as usize]);
            self.tx_buffer
                .push_back(self.joystick_axes[Axis::RightY as usize]);
            self.tx_buffer
                .push_back(self.joystick_axes[Axis::LeftX as usize]);
            self.tx_buffer
                .push_back(self.joystick_axes[Axis::LeftY as usize]);
        }
    }

    fn push_response(&mut self, response: &[u8]) {
        let len = (self.get_response_num_halfword() + 1) * 2;
        self.tx_buffer.clear();
        // for i in 0..len as usize {
        //     self.tx_buffer.push_back(response[i]);
        // }
        self.tx_buffer.extend(&response[0..len as usize]);
    }

    // this is from https://github.com/stenzek/duckstation/blob/master/src/core/analog_controller.cpp#L359
    pub fn get_motor_strength(&self, motor: u8) -> f32 {
        let state = if motor == SMALL_MOTOR {
            if self.motor_state[SMALL_MOTOR as usize] & 0x01 != 0x00 {
                255
            } else {
                0
            }
        } else {
            self.motor_state[LARGE_MOTOR as usize]
        };

        // Curve from https://github.com/KrossX/Pokopom/blob/master/Pokopom/Input_XInput.cpp#L210
        let x = (state as i16 + self.vibration_bias[motor as usize]).clamp(0, 255) as f64;
        let strength = 0.006474549734772402 * x.powi(3) - 1.258165252213538 * x.powi(2)
            + 156.82454281087692 * x
            + 3.637978807091713e-11;
        if state != 0 {
            (strength / 65535.0) as f32
        } else {
            0.0
        }
    }

    pub fn set_ff(&mut self, gilrs: &mut Gilrs) {
        if self.active_gamepad.is_none() {
            for (id, gamepad) in gilrs.gamepads() {
                // info!("{} is {:?} ; ff supported? {}", gamepad.name(), gamepad.power_info(), gamepad.is_ff_supported());
                if gamepad.name().eq("Microsoft Xbox Controller") {
                    self.active_gamepad = Some(id)
                }
            }
        }

        let small_motor_strength = self.get_motor_strength(SMALL_MOTOR);
        let large_motor_strength = self.get_motor_strength(LARGE_MOTOR);
        // self.ff_effect.as_mut().iter().for_each(|x| {
        //     x.stop().unwrap();
        // });
        // println!("go go go go");
        let duration = Ticks::from_ms(18);
        let empty = Ticks::default();

        let effect = EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: (large_motor_strength * 60000.0) as u16,
                },
                scheduling: Replay {
                    play_for: duration,
                    // with_delay: duration * 3,
                    ..Default::default()
                },
                envelope: Default::default(),
            })
            .add_effect(BaseEffect {
                kind: BaseEffectType::Weak {
                    magnitude: (small_motor_strength * 60000.0) as u16,
                },
                scheduling: Replay {
                    after: empty,
                    play_for: duration,
                    with_delay: empty, // with_delay: duration,
                                       // ..Default::default()
                },
                ..Default::default()
            })
            .gamepads(&[self.active_gamepad.unwrap()])
            .finish(gilrs)
            .unwrap();

        effect.play().unwrap();
        self.ff_effect = Some(effect);
    }

    pub fn set_motor_state(&mut self, gilrs: &mut Gilrs, motor: u8, value: u8) {
        if self.motor_state[motor as usize] != value {
            self.motor_state[motor as usize] = value;

            let hvalue = self.get_motor_strength(motor);
            println!(
                "Set {} motor to {} (raw {})",
                if motor == LARGE_MOTOR {
                    "large"
                } else {
                    "small"
                },
                hvalue,
                self.motor_state[motor as usize]
            );
            self.set_ff(gilrs);
            // self.small_motor_strength = hvalue;
        }
    }
}

#[typetag::serde]
impl Controller for Dualshock {
    // Self::Digital => 0x5A41_u16.to_le_bytes(),
    // Self::Analog => 0x5A73_u16.to_le_bytes(),
    // Self::Config => 0x5AF3_u16.to_le_bytes(),
    fn send_and_receive_byte(&mut self, gilrs: &mut Gilrs, data: u8) -> u8 {
        let status_byte = 0x5A; // could be 0x0
        self.rx_buffer.push(data);
        match self.cmd {
            DualshockCommand::Idle => {
                if data == 0x01 {
                    self.cmd = DualshockCommand::Ready;
                    self.in_ack = true;
                    return 0xFF;
                }
                println!("not in ack, got: {}", data);
                self.in_ack = false;
                return 0xFF;
            }

            DualshockCommand::Ready => {
                // Ready
                if data == 0x42 {
                    // println!("read pad");
                    self.cmd = DualshockCommand::ReadPad;
                    self.poll();
                } else if data == 0x43 {
                    self.cmd = DualshockCommand::ConfigModeSetMode;
                    if !self.configuration_mode {
                        self.poll();
                    } else {
                        self.push_response(&[self.get_id_byte(), status_byte, 0, 0, 0, 0, 0, 0]);
                    }
                } else if self.configuration_mode && data == 0x44 {
                    self.cmd = DualshockCommand::SetAnalogueMode;
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                    ]);
                } else if self.configuration_mode && data == 0x45 {
                    self.cmd = DualshockCommand::GetAnalogMode;
                    // println!("get_analogmode..");
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x01,
                        0x02,
                        self.analog_mode as u8,
                        0x02,
                        0x01,
                        0,
                    ]);
                } else if self.configuration_mode && data == 0x46 {
                    self.cmd = DualshockCommand::Command46;
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                    ]);
                } else if self.configuration_mode && data == 0x47 {
                    self.cmd = DualshockCommand::Command47;
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x00,
                        0x00,
                        0x02,
                        0x00,
                        0x01,
                        0x00,
                    ]);
                } else if self.configuration_mode && data == 0x4C {
                    self.cmd = DualshockCommand::Command4C;
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                    ]);
                } else if self.configuration_mode && data == 0x4D {
                    self.cmd = DualshockCommand::GetSetRumble;
                    self.push_response(&[
                        self.get_id_byte(),
                        status_byte,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                    ]);
                } else {
                    if self.configuration_mode {
                        panic!("unhandled command: 0x{:X}", data);
                    }
                    self.in_ack = false;
                    return 0xFF;
                }
            }
            DualshockCommand::ReadPad => {
                // println!("readpad  step: {}", self.cmd_step);
                if self.cmd_step >= 2 && self.cmd_step < 7 {
                    let motor_to_set = self.rumble_config[(self.cmd_step - 2) as usize];
                    if motor_to_set <= LARGE_MOTOR {
                        self.set_motor_state(gilrs, motor_to_set, data);
                    }
                }
            }
            DualshockCommand::ConfigModeSetMode => {
                // println!("config mode step: {}", self.cmd_step);
                if self.tx_buffer.len() == 1 {
                    self.configuration_mode = self.rx_buffer[3] == 1;

                    if self.configuration_mode {
                        //     self.dual_shock_enabled = true;
                        //     self.status = 0x5A;
                    }
                    // println!(
                    //     "0x{:02x}({}) config mode",
                    //     self.rx_buffer[3],
                    //     if self.configuration_mode {
                    //         "enter"
                    //     } else {
                    //         "leave"
                    //     }
                    // );
                    // println!("{:?}", self.rx_buffer);
                }
            }
            DualshockCommand::GetAnalogMode => {
                // println!("analogue mode step: {}", self.cmd_step);
            }
            DualshockCommand::Command4C => {
                // println!("command 4c step: {}", self.cmd_step);
                if self.cmd_step == 2 {
                    if data == 0x00 {
                        self.tx_buffer[3] = 0x04;
                    } else if data == 0x01 {
                        self.tx_buffer[3] = 0x07;
                    }
                }
            }
            DualshockCommand::Command46 => {
                if self.cmd_step == 2 {
                    if data == 0x00 {
                        self.tx_buffer[2] = 0x01;
                        self.tx_buffer[3] = 0x02;
                        self.tx_buffer[4] = 0x00;
                        self.tx_buffer[5] = 0x0A;
                    } else if data == 0x01 {
                        self.tx_buffer[2] = 0x01;
                        self.tx_buffer[3] = 0x01;
                        self.tx_buffer[4] = 0x01;
                        self.tx_buffer[5] = 0x14;
                    }
                }
            }
            DualshockCommand::Command47 => {
                if (self.cmd_step == 2 && data != 0x00) {
                    self.tx_buffer[2] = 0x00;
                    self.tx_buffer[3] = 0x00;
                    self.tx_buffer[4] = 0x00;
                    self.tx_buffer[5] = 0x00;
                }
            }
            DualshockCommand::SetAnalogueMode => {
                if self.cmd_step == 2 {
                    println!("analog mode val 0x{:02x}", data);

                    if (data == 0x00 || data == 0x01) {
                        self.set_analog_mode((data == 0x01));
                    }
                } else if self.cmd_step == 3 {
                    println!("analog mode lock 0x{:02x}", data);
                    if (data == 0x02 || data == 0x03) {
                        self.analog_locked = (data == 0x03);
                    }
                }
            }
            DualshockCommand::GetSetRumble => {
                if self.cmd_step >= 2 && self.cmd_step < 7 {
                    let index = self.cmd_step - 2;
                    self.tx_buffer[0] = self.rumble_config[index as usize];
                    self.rumble_config[index as usize] = data;
                } else if self.cmd_step == 7 {
                    // reset motor value if we're no longer mapping it
                    let mut has_small = false;
                    let mut has_large = false;
                    for i in 0..self.rumble_config.len() {
                        has_small |= self.rumble_config[i] == SMALL_MOTOR;
                        has_large |= self.rumble_config[i] == LARGE_MOTOR;
                    }
                    if !has_small {
                        self.set_motor_state(gilrs, SMALL_MOTOR, 0);
                    }
                    if !has_large {
                        self.set_motor_state(gilrs, LARGE_MOTOR, 0);
                    }
                }
            }

        };

        // if let Some(state) = self.mode.next(self.state, data) {
        //     self.state = state;
        //     self.in_ack = state != DualshockState::Init;
        //     received
        // } else {
        //     self.reset();
        //     0xFF
        // }
        let response = self.tx_buffer.pop_front().expect("smth");
        self.cmd_step += 1;
        self.in_ack = !self.tx_buffer.is_empty();
        if self.tx_buffer.is_empty() {
            // println!("end command at {}", self.cmd_step);
            // println!("rx buffer: {:?}", self.rx_buffer);
            self.cmd = DualshockCommand::Idle;
            self.cmd_step = 0;
            self.rx_buffer.clear();
            self.in_ack = false;
        }
        return response;
    }

    fn in_ack(&self) -> bool {
        self.in_ack
    }

    fn reset(&mut self) {
        // println!("reset");
        self.in_ack = false;
        self.cmd_step = 0;
        self.rx_buffer.clear();
        // self.configuration_mode = false;
        // self.analog_mode = false;
        self.cmd = DualshockCommand::Idle;
    }

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    fn get_stick_axis(&mut self) -> &mut [u8; 4] {
        &mut self.joystick_axes
    }

    fn get_buttons(&self) -> u16 {
        self.digital_switches
    }

    fn set_analog_mode(&mut self, is_analog: bool) {
        if self.analog_mode == is_analog {
            return;
        }
        self.analog_mode = is_analog;
        self.reset();
    }
    fn get_analog_mode(&self) -> bool {
        self.analog_mode
    }

    fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for Dualshock {
    fn default() -> Self {
        Self {
            cmd: DualshockCommand::Idle,
            tx_buffer: VecDeque::new(),
            rx_buffer: vec![],
            cmd_step: 0,
            analog_mode: false,
            configuration_mode: false,
            digital_switches: 0xFFFF,
            joystick_axes: [0x80; 4],
            in_ack: Default::default(),
            rumble_config: [0x0; 6],
            motor_state: [0x0; 2],
            vibration_bias: [8, 8],
            ff_effect: None,
            active_gamepad: None,
            analog_locked: false,
        }
    }
}

const FRAME_SIZE: usize = 0x80;

#[derive(Serialize, Deserialize)]
pub struct Memcard {
    in_ack: bool,
    state: MemcardState,
    command: MemcardCommand,

    state_idx: usize,
    sector_number: u16,
    checksum: u8,

    #[serde(with = "serde_bytes")]
    sector_buffer: [u8; 128],
    bytes_left: usize,

    end_response: EndResponse,
    directory_not_read: bool,

    is_dirty: bool,
    #[serde(skip)]
    data: Box<[u8]>, // 0x20000
    #[serde(skip)]
    pub path: String, // file: Option<File>,
}

impl Memcard {
    pub fn new(path: &str) -> Self {
        // let file = File::open(path).expect("Inalid file path for memcard");
        let mut data = vec![0_u8; 0x20000].into_boxed_slice();
        let filedata = std::fs::read(path).expect("can't read memcard");
        data.copy_from_slice(&filedata);
        Memcard {
            in_ack: false,
            state: Default::default(),
            command: MemcardCommand::Read,
            state_idx: 0,
            sector_number: 0,
            checksum: 0,
            sector_buffer: [0; 128],
            bytes_left: 0,
            end_response: EndResponse::Good,
            directory_not_read: true,
            is_dirty: false,
            path: path.to_string(),
            // file: Some(file),
            data,
        }
    }

    pub fn read_file_data(&mut self) {
        // let data =  vec![0_u8; 0x20000].into_boxed_slice();
        let filedata = std::fs::read(&self.path).expect("can't read memcard");
        self.data = filedata.into_boxed_slice();
    }

    pub fn send_and_receive_byte(&mut self, data: u8) -> u8 {
        let send = match self.state {
            MemcardState::Init => 0xFF,
            MemcardState::CardId1 => 0x5A,
            MemcardState::CardId2 => 0x5D,
            MemcardState::CmdAck2 => 0x5D,
            MemcardState::CmdAck1 => 0x5C,
            MemcardState::Recv04h => 0x04,
            MemcardState::Recv00h => 0x00,
            MemcardState::Recv80h => 0x80,
            MemcardState::AckMsb => (self.sector_number >> 8) as u8,
            MemcardState::AckLsb => (self.sector_number & 0xFF) as u8,
            MemcardState::Flag => u8::from(self.directory_not_read) << 3,

            MemcardState::SendMsb => {
                self.sector_number = u16::from(data) << 8;
                self.checksum = data;
                0x00
            }

            MemcardState::SendLsb => {
                self.sector_number |= u16::from(data);
                self.checksum ^= data;

                if self.sector_number > 0x3FF {
                    println!("memcard invalid sector address. aborting");
                    self.sector_number = 0xFFFF;
                    self.end_response = EndResponse::BadSector;
                }

                0x00
            }

            MemcardState::RecvSector => {
                if data != 0 {
                    println!("memcard recv sector. unexpected data from host");
                }
                let byte = self.sector_buffer[128 - self.bytes_left];
                self.bytes_left -= 1;
                self.checksum ^= byte;

                if self.bytes_left > 0 {
                    return byte;
                }

                byte
            }

            MemcardState::SendSector => {
                self.sector_buffer[128 - self.bytes_left] = data;
                self.bytes_left -= 1;
                self.checksum ^= data;

                if self.bytes_left > 0 {
                    return 0;
                }

                0
            }

            MemcardState::RecvChecksum => self.checksum,
            MemcardState::SendChecksum => {
                self.end_response = if self.checksum == data {
                    EndResponse::Good
                } else {
                    EndResponse::BadChecksum
                };
                0
            }
            MemcardState::MemEnd => {
                if self.command == MemcardCommand::Write {
                    self.save_sector();
                }
                self.directory_not_read = false;
                self.end_response as u8
            }
        };

        if let Some((next_state, next_idx)) = self.command.next(self.state, data, self.state_idx) {
            if matches!(next_state, MemcardState::RecvSector) {
                self.load_sector();
                self.bytes_left = 128;
            }
            if matches!(next_state, MemcardState::SendSector) {
                self.bytes_left = 128;
            }

            self.state = next_state;
            self.state_idx = next_idx;
            self.in_ack = self.state != MemcardState::Init;

            send
        } else {
            self.reset();
            0xFF
        }
    }
    pub fn in_ack(&self) -> bool {
        self.in_ack
    }

    pub fn load_sector(&mut self) {
        let address = (self.sector_number as usize) * FRAME_SIZE;
        self.sector_buffer
            .copy_from_slice(&self.data[address..address + 128]);
    }

    pub fn save_sector(&mut self) {
        let address = (self.sector_number as usize) * FRAME_SIZE;
        self.data[address..address + 128].copy_from_slice(&self.sector_buffer);
        self.is_dirty = true;
    }

    pub fn reset(&mut self) {
        self.in_ack = false;
        self.state = MemcardState::Init;
        self.state_idx = 0;
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MemcardState {
    #[default]
    Init,
    Flag,
    CardId1,
    CardId2,
    CmdAck1,
    CmdAck2,
    Recv04h,
    Recv00h,
    Recv80h,
    SendMsb,
    SendLsb,
    SendSector,
    RecvSector,
    SendChecksum,
    RecvChecksum,
    AckMsb,
    AckLsb,
    MemEnd,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[repr(C)]
pub enum EndResponse {
    Good = 0x47,
    BadChecksum = 0x4E,
    BadSector = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MemcardCommand {
    Read,
    Write,
    GetId,
}

impl MemcardCommand {
    const GETID_STATES: [(MemcardState, Option<u8>); 10] = [
        (MemcardState::Init, None),
        (MemcardState::Flag, Some(0x81)),
        (MemcardState::CardId1, Some(0x53)),
        (MemcardState::CardId2, Some(0x00)),
        (MemcardState::CmdAck1, Some(0x00)),
        (MemcardState::CmdAck2, Some(0x00)),
        (MemcardState::Recv04h, Some(0x00)),
        (MemcardState::Recv00h, Some(0x00)),
        (MemcardState::Recv00h, Some(0x00)),
        (MemcardState::Recv80h, Some(0x00)),
    ];

    const READ_STATES: [(MemcardState, Option<u8>); 13] = [
        (MemcardState::Init, None),
        (MemcardState::Flag, Some(0x81)),
        (MemcardState::CardId1, Some(0x52)),
        (MemcardState::CardId2, Some(0x00)),
        (MemcardState::SendMsb, Some(0x00)),
        (MemcardState::SendLsb, None),
        (MemcardState::CmdAck1, None),
        (MemcardState::CmdAck2, Some(0x00)),
        (MemcardState::AckMsb, Some(0x00)),
        (MemcardState::AckLsb, Some(0x00)),
        (MemcardState::RecvSector, Some(0x00)), // 128 bytes
        (MemcardState::RecvChecksum, Some(0x00)),
        (MemcardState::MemEnd, Some(0x00)),
    ];
    const WRITE_STATES: [(MemcardState, Option<u8>); 11] = [
        (MemcardState::Init, None),
        (MemcardState::Flag, Some(0x81)),
        (MemcardState::CardId1, Some(0x57)),
        (MemcardState::CardId2, Some(0x00)),
        (MemcardState::SendMsb, Some(0x00)),
        (MemcardState::SendLsb, None),
        (MemcardState::SendSector, None),   // 128 bytes
        (MemcardState::SendChecksum, None), // MSB xor LSB xor Data bytes
        (MemcardState::CmdAck1, None),
        (MemcardState::CmdAck2, Some(0x00)),
        (MemcardState::MemEnd, Some(0x00)),
    ];
    const fn states_table(self) -> &'static [(MemcardState, Option<u8>)] {
        match self {
            Self::Read => &Self::READ_STATES,
            Self::Write => &Self::WRITE_STATES,
            Self::GetId => &Self::GETID_STATES,
        }
    }

    pub fn next(
        &mut self,
        current: MemcardState,
        recv: u8,
        state_idx: usize,
    ) -> Option<(MemcardState, usize)> {
        if current == MemcardState::Flag {
            *self = match recv {
                0x52 => Self::Read,
                0x57 => Self::Write,
                0x53 => Self::GetId,
                _ => return None,
            }
        }
        let table = self.states_table();
        let next_idx = (state_idx + 1) % table.len();
        let (next_state, check_byte) = table[next_idx];

        let valid = check_byte.is_none_or(|b| b == recv);
        valid.then_some((next_state, next_idx))
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum GamepadState {
    #[default]
    Init,
    IdLow,
    IdHigh,
    SwitchLow,
    SwitchHigh,
    AnalogInput0,
    AnalogInput1,
    AnalogInput2,
    AnalogInput3,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Copy, Clone)]
pub enum Button {
    Select,
    L3,
    R3,
    Start,
    Up,
    Right,
    Down,
    Left,
    L2,
    R2,
    L1,
    R1,
    Square,
    Circle,
    Cross,
    Triangle,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum Axis {
    RightX,
    RightY,
    LeftX,
    LeftY,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GamepadMode {
    #[default]
    Digital,
    Analog,
    Config,
}

impl GamepadMode {
    const DIGITAL_STATES: [(GamepadState, Option<u8>); 5] = [
        (GamepadState::Init, None),
        (GamepadState::IdLow, Some(0x01)),
        (GamepadState::IdHigh, Some(0x42)),
        (GamepadState::SwitchLow, None),
        (GamepadState::SwitchHigh, None),
    ];
    const ANALOG_STATES: [(GamepadState, Option<u8>); 9] = [
        (GamepadState::Init, Some(0x00)),
        (GamepadState::IdLow, Some(0x01)),
        (GamepadState::IdHigh, Some(0x42)),
        (GamepadState::SwitchLow, None),
        (GamepadState::SwitchHigh, None),
        (GamepadState::AnalogInput0, None),
        (GamepadState::AnalogInput1, Some(0x00)),
        (GamepadState::AnalogInput2, Some(0x00)),
        (GamepadState::AnalogInput3, Some(0x00)),
    ];

    const fn id(self) -> [u8; 2] {
        match self {
            Self::Digital => 0x5A41_u16.to_le_bytes(),
            Self::Analog => 0x5A73_u16.to_le_bytes(),
            Self::Config => 0x5AF3_u16.to_le_bytes(),
            // Self::Analog => 0x5A79_u16.to_le_bytes(),// dual shock 2, doesn't work like this
        }
    }
    pub fn set(&mut self, in_analog: bool) {
        *self = if in_analog {
            Self::Analog
        } else {
            Self::Digital
        };
    }

    pub fn get(self) -> bool {
        match self {
            Self::Digital => false,
            Self::Analog => true,
            Self::Config => true,
        }
    }

    const fn state_table(self) -> &'static [(GamepadState, Option<u8>)] {
        match self {
            Self::Digital => &Self::DIGITAL_STATES,
            Self::Analog => &Self::ANALOG_STATES,
            Self::Config => &Self::ANALOG_STATES,
        }
    }

    fn next(self, current_state: GamepadState, received_byte: u8) -> Option<GamepadState> {
        let idx = current_state as usize;
        let state_table = self.state_table();

        let (next_state, check_byte) = state_table[(idx + 1) % state_table.len()];
        // if current_state == GamepadState::IdLow {
        // if received_byte == 0x43 {
        //     panic!("gotcha!")
        // }
        // ok,  next byte checks if we want
        // so it works..
        // println!("data: {:X}, next_state: {:?}, check_byte: {:?}", received_byte, next_state, check_byte);
        // }
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

pub fn save_memcards(system: &mut std::pin::Pin<Box<System>>) {
    if system.sio.memcard1.is_dirty {
        std::fs::write(&system.sio.memcard1.path, &system.sio.memcard1.data).expect("ok");
        system.sio.memcard1.is_dirty = false;
    }
    if system.sio.memcard2.is_dirty {
        std::fs::write(&system.sio.memcard2.path, &system.sio.memcard2.data).expect("ok");
        system.sio.memcard2.is_dirty = false;
    }
}

const SMALL_MOTOR: u8 = 0;
const LARGE_MOTOR: u8 = 1;
//
// #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
// pub enum DualShockMode {
//     #[default]
//     Normal,
//     Config,
// }
// #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
// pub enum AnalogMode {
//     #[default]
//     Digital,
//     Analog,
// }
//
// impl AnalogMode {
//     pub fn toggle(self) -> Self {
//         match self {
//             Self::Digital => Self::Analog,
//             Self::Analog => Self::Digital,
//         }
//     }
// }
//
// #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// pub struct DualShockControllerState {
//     pub mode: DualShockMode,
//     pub analog_mode: AnalogMode,
//     pub rumble_config: [u8; 6],
//     pub analog_mode_locked: bool,
//     pub config_mode_entered: bool,
//     pub analog_mode_changed: bool,
// }
//
// impl Default for DualShockControllerState {
//     fn default() -> Self {
//         Self {
//             mode: DualShockMode::default(),
//             analog_mode: AnalogMode::default(),
//             rumble_config: [0xFF; 6],
//             analog_mode_locked: false,
//             config_mode_entered: false,
//             analog_mode_changed: false,
//         }
//     }
// }
//
// impl DualShockControllerState {
//     pub fn toggle_analog_mode(&mut self) {
//         if !self.analog_mode_locked {
//             self.analog_mode = self.analog_mode.toggle();
//             self.analog_mode_changed = true;
//             self.rumble_config.fill(0xFF);
//         }
//     }
// }
//
// #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
// pub enum DualShockConfigCommand {
//     // 0x43
//     ChangeMode,
//     // 0x44
//     SetLedState,
//     // 0x45
//     GetLedState,
//     // 0x46
//     VariableResponseA,
//     // 0x47
//     ConstantValues,
//     // 0x4C
//     VariableResponseB,
//     // 0x4D
//     ConfigureRumble,
// }
//
// #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
// pub enum DualShockSioState {
//     SendingIdLow,
//     NormalSendingIdHigh { entering_config: bool },
//     ConfigSendingIdHigh(DualShockConfigCommand),
//     SendingDigitalInputsLow { entering_config: bool },
//     SendingDigitalInputsHigh { send_analog: bool },
//     SendingAnalogInputsRightX,
//     SendingAnalogInputsRightY,
//     SendingAnalogInputsLeftX,
//     SendingAnalogInputsLeftY,
//     ConfigReceivingModeChange,
//     ReceivingLedState,
//     ReceivingLedLockState,
//     SendingLedState { remaining: u8 },
//     ConfigConstantValues { remaining: u8 },
//     VariableResponseAReceivingInput,
//     SendingVariableResponseAZero { remaining: u8 },
//     SendingVariableResponseAOne { remaining: u8 },
//     VariableResponseBReceivingInput,
//     SendingVariableResponseB { remaining: u8, input: u8 },
//     ConfiguringRumble { remaining: u8 },
//     SendingZeroes { remaining: u8 },
// }
//
// // bitfield! {
// //     #[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
// //     pub struct DigitalJoypadState(u16): Debug {
// //         pub select: bool @ 0,
// //         pub start: bool @ 3,
// //         pub up: bool @ 4,
// //         pub right: bool @ 5,
// //         pub down: bool @ 6,
// //         pub left: bool @ 7,
// //         pub l2: bool @ 8,
// //         pub r2: bool @ 9,
// //         pub l1: bool @ 10,
// //         pub r1: bool @ 11,
// //         pub triangle: bool @ 12,
// //         pub circle: bool @ 13,
// //         pub cross: bool @ 14,
// //         pub square: bool @ 15,
// //     }
// // }
//
// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct DualShock {
//     state: DualShockSioState,
//
//     pub digital_switches: u16,
//     pub joystick_axes: [u8; 4],
//
//     in_ack: bool,
//     rx: VecDeque<u8>,
//
//     controller_state: DualShockControllerState,
// }
//
// #[typetag::serde]
// impl Controller for DualShock {
//     fn send_and_receive_byte(&mut self, gilrs: &mut Gilrs, data: u8) -> u8 {
//         if !self.in_ack {
//             if data == 1 {
//                 self.in_ack = true;
//                 return 0xFF;
//             } else {
//                 self.in_ack = false;
//                 return 0xFF;
//             }
//         }
//         self.process(data);
//         if self.rx.is_empty() {
//             self.in_ack = false;
//             0xFF
//         } else {
//             self.rx.pop_front().expect("ok")
//         }
//     }
//
//     fn in_ack(&self) -> bool {
//         self.in_ack
//     }
//
//     fn reset(&mut self) {
//         self.in_ack = false;
//         self.state = DualShockSioState::SendingIdLow;
//         self.rx.clear();
//     }
//
//     fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
//         self.joystick_axes = [right.0, right.1, left.0, left.1];
//     }
//
//     fn get_stick_axis(&mut self) -> &mut [u8; 4] {
//         &mut self.joystick_axes
//     }
//
//     fn set_buttons(&mut self, buttons: u16) {
//         self.digital_switches = buttons;
//     }
//
//     fn get_buttons(&self) -> u16 {
//         self.digital_switches
//         // todo!()
//     }
//
//     fn set_analog_mode(&mut self, is_analog: bool) {
//         todo!()
//     }
//
//     fn get_analog_mode(&self) -> bool {
//         todo!()
//     }
//
//     fn as_any(&mut self) -> &mut dyn Any {
//         self
//     }
// }
//
// impl DualShock {
//     pub fn initial() -> Self {
//         Self {
//             state: DualShockSioState::SendingIdLow,
//             digital_switches: 0xFFFF,
//             joystick_axes: [0x80; 4],
//             in_ack: false,
//             rx: VecDeque::new(),
//             controller_state: Default::default(),
//         }
//     }
//
//     pub fn process(&mut self, tx: u8) {
//         use DualShockConfigCommand as ConfigCommand;
//         use DualShockSioState as SioState;
//
//         log::debug!("Received tx byte {tx:02X}, current state {:?}", self.state);
//
//         match self.state {
//             SioState::SendingIdLow => {
//                 let id_low = match (
//                     self.controller_state.mode,
//                     self.controller_state.analog_mode,
//                 ) {
//                     (DualShockMode::Normal, AnalogMode::Digital) => 0x41,
//                     (DualShockMode::Normal, AnalogMode::Analog) => 0x73,
//                     (DualShockMode::Config, _) => 0xF3,
//                 };
//                 self.rx.push_back(id_low);
//
//                 match (self.controller_state.mode, tx) {
//                     (_, 0x42) => self.set_state(SioState::NormalSendingIdHigh {
//                         entering_config: false,
//                     }),
//                     (DualShockMode::Normal, 0x43) => {
//                         (self.set_state(SioState::NormalSendingIdHigh {
//                             entering_config: true,
//                         }))
//                     }
//                     (DualShockMode::Config, 0x43) => {
//                         self.set_state(SioState::ConfigSendingIdHigh(ConfigCommand::ChangeMode))
//                     }
//                     (DualShockMode::Config, 0x44) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(ConfigCommand::SetLedState)))
//                     }
//                     (DualShockMode::Config, 0x45) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(ConfigCommand::GetLedState)))
//                     }
//                     (DualShockMode::Config, 0x46) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(
//                             ConfigCommand::VariableResponseA,
//                         )))
//                     }
//                     (DualShockMode::Config, 0x47) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(
//                             ConfigCommand::ConstantValues,
//                         )))
//                     }
//                     (DualShockMode::Config, 0x4C) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(
//                             ConfigCommand::VariableResponseB,
//                         )))
//                     }
//                     (DualShockMode::Config, 0x4D) => {
//                         (self.set_state(SioState::ConfigSendingIdHigh(
//                             ConfigCommand::ConfigureRumble,
//                         )))
//                     }
//                     _ => {}
//                 }
//             }
//             SioState::NormalSendingIdHigh { entering_config } => {
//                 // DualShock sends 0x00 after analog mode is changed to signal to games that they
//                 // may need to reconfigure the controller
//                 self.rx.push_back(
//                     if self.controller_state.analog_mode_changed
//                         && self.controller_state.config_mode_entered
//                     {
//                         0x00
//                     } else {
//                         0x5A
//                     },
//                 );
//
//                 (self.set_state(SioState::SendingDigitalInputsLow { entering_config }))
//             }
//             SioState::ConfigSendingIdHigh(command) => {
//                 // ID high byte is always 0x5A
//                 self.rx.push_back(0x5A);
//
//                 (match command {
//                     ConfigCommand::ChangeMode => {
//                         self.set_state(SioState::ConfigReceivingModeChange)
//                     }
//                     ConfigCommand::SetLedState => self.set_state(SioState::ReceivingLedState),
//                     ConfigCommand::GetLedState => {
//                         self.set_state(SioState::SendingLedState { remaining: 6 })
//                     }
//                     ConfigCommand::VariableResponseA => {
//                         self.set_state(SioState::VariableResponseAReceivingInput)
//                     }
//                     ConfigCommand::ConstantValues => {
//                         self.set_state(SioState::ConfigConstantValues { remaining: 6 })
//                     }
//                     ConfigCommand::VariableResponseB => {
//                         self.set_state(SioState::VariableResponseBReceivingInput)
//                     }
//                     ConfigCommand::ConfigureRumble => {
//                         self.set_state(SioState::ConfiguringRumble { remaining: 6 })
//                     }
//                 })
//             }
//             SioState::SendingDigitalInputsLow { entering_config } => {
//                 let send_analog = self.controller_state.mode == DualShockMode::Config
//                     || self.controller_state.analog_mode == AnalogMode::Analog;
//
//                 let mut digital_low = u16::from(self.digital_switches) as u8;
//                 if send_analog {
//                     // L3 and R3 are only sent in analog mode (or if entering config mode)
//                     // digital_low |=
//                     //     (u8::from(self.analog.l3) << 1) | (u8::from(self.analog.r3) << 2);
//                 }
//                 self.rx.push_back(!digital_low);
//
//                 if entering_config && tx == 0x01 {
//                     self.controller_state.mode = DualShockMode::Config;
//                     self.controller_state.analog_mode_changed = false;
//                     self.controller_state.config_mode_entered = true;
//                 }
//
//                 (self.set_state(SioState::SendingDigitalInputsHigh { send_analog }))
//             }
//             SioState::SendingDigitalInputsHigh { send_analog } => {
//                 self.rx
//                     .push_back((!u16::from(self.digital_switches) >> 8) as u8);
//
//                 if send_analog {
//                     self.set_state(SioState::SendingAnalogInputsRightX)
//                 }
//             }
//             SioState::SendingAnalogInputsRightX => {
//                 self.rx.push_back(self.joystick_axes[0]);
//                 (self.set_state(SioState::SendingAnalogInputsRightY))
//             }
//             SioState::SendingAnalogInputsRightY => {
//                 self.rx.push_back(self.joystick_axes[1]);
//                 (self.set_state(SioState::SendingAnalogInputsLeftX))
//             }
//             SioState::SendingAnalogInputsLeftX => {
//                 self.rx.push_back(self.joystick_axes[2]);
//                 (self.set_state(SioState::SendingAnalogInputsLeftY))
//             }
//             SioState::SendingAnalogInputsLeftY => {
//                 self.rx.push_back(self.joystick_axes[3]);
//             }
//             SioState::ConfigReceivingModeChange => {
//                 self.rx.push_back(0x00);
//
//                 if tx == 0x00 {
//                     self.controller_state.mode = DualShockMode::Normal;
//                 }
//
//                 (self.set_state(SioState::SendingZeroes { remaining: 5 }))
//             }
//             SioState::ReceivingLedState => {
//                 self.rx.push_back(0x00);
//
//                 match tx {
//                     0x00 => {
//                         self.controller_state.analog_mode = AnalogMode::Digital;
//                         log::debug!("Analog mode forcibly disabled");
//                     }
//                     0x01 => {
//                         self.controller_state.analog_mode = AnalogMode::Analog;
//                         log::debug!("Analog mode forcibly enabled");
//                     }
//                     _ => {}
//                 }
//
//                 (self.set_state(SioState::ReceivingLedLockState))
//             }
//             SioState::ReceivingLedLockState => {
//                 self.rx.push_back(0x00);
//
//                 self.controller_state.analog_mode_locked = tx & 0x03 == 0x03;
//                 log::debug!(
//                     "Analog mode locked: {}",
//                     self.controller_state.analog_mode_locked
//                 );
//
//                 (self.set_state(SioState::SendingZeroes { remaining: 4 }))
//             }
//             SioState::SendingLedState { remaining } => {
//                 self.rx.push_back(match remaining {
//                     6 | 2 => 0x01,
//                     5 | 3 => 0x02,
//                     4 => match self.controller_state.analog_mode {
//                         AnalogMode::Digital => 0x00,
//                         AnalogMode::Analog => 0x01,
//                     },
//                     1 => 0x00,
//                     _ => panic!("Invalid DualShock state: {:?}", self.state),
//                 });
//
//                 if (remaining > 1) {
//                     self.set_state(SioState::SendingLedState {
//                         remaining: remaining - 1,
//                     });
//                 }
//             }
//             SioState::ConfigConstantValues { remaining } => {
//                 self.rx.push_back(match remaining {
//                     4 => 0x02,
//                     2 => 0x01,
//                     _ => 0x00,
//                 });
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::ConfigConstantValues {
//                         remaining: remaining - 1,
//                     }));
//                 }
//             }
//             SioState::VariableResponseAReceivingInput => {
//                 self.rx.push_back(0x00);
//
//                 (match tx {
//                     0x00 => self.set_state(SioState::SendingVariableResponseAZero { remaining: 5 }),
//                     0x01 => self.set_state(SioState::SendingVariableResponseAOne { remaining: 5 }),
//                     _ => self.set_state(SioState::SendingZeroes { remaining: 5 }),
//                 });
//             }
//             SioState::SendingVariableResponseAZero { remaining } => {
//                 self.rx.push_back(match remaining {
//                     5 | 2 => 0x00,
//                     4 => 0x01,
//                     3 => 0x02,
//                     1 => 0x0A,
//                     _ => panic!("Invalid DualShock state: {:?}", self.state),
//                 });
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::SendingVariableResponseAZero {
//                         remaining: remaining - 1,
//                     }));
//                 }
//             }
//             SioState::SendingVariableResponseAOne { remaining } => {
//                 self.rx.push_back(match remaining {
//                     5 => 0x00,
//                     2..=4 => 0x01,
//                     1 => 0x14,
//                     _ => panic!("Invalid DualShock state: {:?}", self.state),
//                 });
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::SendingVariableResponseAOne {
//                         remaining: remaining - 1,
//                     }));
//                 }
//             }
//             SioState::VariableResponseBReceivingInput => {
//                 self.rx.push_back(0x00);
//                 (self.set_state(SioState::SendingVariableResponseB {
//                     remaining: 5,
//                     input: tx,
//                 }))
//             }
//             SioState::SendingVariableResponseB { remaining, input } => {
//                 if remaining == 3 {
//                     self.rx.push_back(match input {
//                         0x00 => 0x04,
//                         0x01 => 0x07,
//                         _ => 0x00,
//                     });
//                 } else {
//                     self.rx.push_back(0x00);
//                 }
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::SendingVariableResponseB {
//                         remaining: remaining - 1,
//                         input,
//                     }));
//                 }
//             }
//             SioState::SendingZeroes { remaining } => {
//                 self.rx.push_back(0x00);
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::SendingZeroes {
//                         remaining: remaining - 1,
//                     }));
//                 }
//             }
//             SioState::ConfiguringRumble { remaining } => {
//                 // Rumble configure command returns the previous values as it accepts new ones
//                 let idx = (6 - remaining) as usize;
//                 self.rx.push_back(self.controller_state.rumble_config[idx]);
//                 self.controller_state.rumble_config[idx] = tx;
//
//                 if (remaining > 1) {
//                     (self.set_state(SioState::ConfiguringRumble {
//                         remaining: remaining - 1,
//                     }));
//                 }
//             }
//         }
//     }
//
//     fn set_state(&mut self, state: DualShockSioState) {
//         self.state = state;
//     }
// }

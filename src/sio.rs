use std::{any::Any, fs::File};

use arrayvec::ArrayVec;
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
    pub fn new(negcon: bool, mouse1: bool, mouse2:bool, guncon: bool, justifier: bool) -> Self {
        let gamepad1: Box<dyn Controller> = if justifier {
            Box::new(Justifier::default())
        } else if guncon {
            Box::new(GunCon::default())
        } else if mouse1 {
            Box::new(Mouse::default())
        } else if negcon {
            Box::new(NegCon::default())
        } else {
            Box::new(Gamepad::default())
        };
        let gamepad2: Box<dyn Controller> = if mouse2 {
            Box::new(Mouse::default())
        } else {
            Box::new(Gamepad::default())
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
                sio.mode.0 = val &  0x1FF;
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

    pub fn send_and_receive_byte(&mut self, data: u8) -> (u8, bool) {
        let port = usize::from(self.control.port_select());

        let (byte, next_state) = match self.state {
            State::None => match data {
                0x01 => self.process_gamepad(port, data),
                0x81 => self.process_memcard(port, data),
                _ => (0xFF, State::None),
            },
            State::GamepadComm => self.process_gamepad(port, data),
            State::MemcardComm => self.process_memcard(port, data),
        };
        self.state = next_state;
        (byte, next_state != State::None)
    }

    // let's start with one gamepad and one memcard for now...
    pub fn process_gamepad(&mut self, port: usize, data: u8) -> (u8, State) {
        if port == 0 {
            let byte = self.gamepad1.send_and_receive_byte(data);
            let state = if self.gamepad1.in_ack() {
                State::GamepadComm
            } else {
                State::None
            };
            (byte, state)
        } else {
            let byte = self.gamepad2.send_and_receive_byte(data);
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
            let (received, ack) = system.sio.send_and_receive_byte(val);

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
    fn send_and_receive_byte(&mut self, data: u8) -> u8;
    fn in_ack(&self) -> bool;

    fn reset(&mut self);

    fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8));

    fn get_stick_axis(&mut self) -> &mut[u8;4];

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
    fn send_and_receive_byte(&mut self, data: u8) -> u8 {
        const ID: [u8;2] = 0x5A23_u16.to_le_bytes();
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

    fn get_stick_axis(&mut self) -> &mut [u8;4] {
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
    fn send_and_receive_byte(&mut self, data: u8) -> u8 {
        const ID: [u8;2] = 0x5A12_u16.to_le_bytes();
        let received = match self.state {
            MouseState::Init => 0xFF,

            MouseState::IdLow => ID[0],
            MouseState::IdHigh => ID[1],

            MouseState::SwitchLow => self.digital_switches as u8, // not used
            MouseState::SwitchHigh => (self.digital_switches >> 8)  as u8, // 10 right, 11 left

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

    fn get_stick_axis(&mut self) -> &mut [u8;4] {
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
            joystick_axes: [0x0d,0x01, 0x88, 0x00] ,
            in_ack: Default::default(),
            shoot_offscreen: false
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
    fn send_and_receive_byte(&mut self, data: u8) -> u8 {
        const ID: [u8;2] = 0x5A63_u16.to_le_bytes();
        let axes = if self.shoot_offscreen {
            &[0x01,0x00, 0xa, 0x00]
        } else {
            &self.joystick_axes
        };
        let received = match self.state {
            GunConState::Init => 0xFF,

            GunConState::IdLow => ID[0],
            GunConState::IdHigh => ID[1],

            GunConState::SwitchLow => self.digital_switches as u8, // not used
            GunConState::SwitchHigh => (self.digital_switches >> 8)  as u8, // 10 right, 11 left

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

    fn get_stick_axis(&mut self) -> &mut [u8;4] {
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
            joystick_axes: [0x0,0x0, 0x0, 0x0] ,
            in_ack: Default::default(),
            shoot_offscreen: false
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
    fn send_and_receive_byte(&mut self, data: u8) -> u8 {
        const ID: [u8;2] = 0x5A31_u16.to_le_bytes();
        let received = match self.state {
            JustifierState::Init => 0xFF,

            JustifierState::IdLow => ID[0],
            JustifierState::IdHigh => ID[1],

            JustifierState::SwitchLow => {
                self.digital_switches as u8
            },
            JustifierState::SwitchHigh => {
                (self.digital_switches >> 8)  as u8
            }, 

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

    fn get_stick_axis(&mut self) -> &mut [u8;4] {
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
    fn send_and_receive_byte(&mut self, data: u8) -> u8 {
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

    fn get_stick_axis(&mut self) -> &mut [u8;4] {
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
    pub path: String
    // file: Option<File>,
}

impl Memcard {
    pub fn new(path: &str) -> Self {
        // let file = File::open(path).expect("Inalid file path for memcard");
        let mut data =  vec![0_u8; 0x20000].into_boxed_slice();
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
            },

            MemcardState::SendLsb => {
                self.sector_number |= u16::from(data);
                self.checksum ^= data;

                if self.sector_number > 0x3FF {
                    println!("memcard invalid sector address. aborting");
                    self.sector_number = 0xFFFF;
                    self.end_response = EndResponse::BadSector;
                }

                0x00
            },

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
            },

            MemcardState::SendSector => {
                self.sector_buffer[128 - self.bytes_left] = data;
                self.bytes_left -= 1;
                self.checksum ^= data;

                if self.bytes_left > 0 {
                    return 0;
                }

                0
            },

            MemcardState::RecvChecksum => self.checksum,
            MemcardState::SendChecksum => {
                self.end_response = if self.checksum == data {
                    EndResponse::Good
                } else {
                    EndResponse::BadChecksum
                };
                0
            },
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
    Config
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

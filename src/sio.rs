use arrayvec::ArrayVec;

use crate::memory_bus::{Addressable, MemoryBus};


pub struct Sio {
    control: Control,
    boudrate_reload: u16,
    mode: Mode,
    status: Status,
    transfer: Option<u8>,
    received: ArrayVec<u8,8>,

    // move out?
    state: State,
    pub gamepad: Gamepad,
    pub memcard: Memcard,
}

impl Sio {

    pub fn new() -> Self {
        Sio {
            control: Control::default(),
            boudrate_reload: 0,
            mode: Mode::default(),
            status: Status(0x22005), // TX idle and TX ready
            transfer: None,
            received: ArrayVec::new(),
            state: State::None,
            gamepad: Gamepad::default(),
            memcard: Memcard::default(),
        }
    }

    pub fn load<T:Addressable>(&mut self, offset: u32) -> T {
        match offset {
            0x0 => T::from_u32(self.pop_received_data()),
            0x4 => {
                // self.status.set_tx_fifo_not_full(true);
                // self.status.set_tx_idle(true);
                // self.status.set_rx_fifo_not_empty(true);
                T::from_u32(self.status.0)},
            0xA => {T::from_u32(self.control.0 as u32)},
            _   => panic!("Unhandled sio load{:?} offset: {:08x}", T::width(), offset)
        }
    }

    pub fn store<T:Addressable>(&mut self, offset: u32, value: T) {
        // 1F801040h+N*10h - SIO#_TX_DATA (W)
        let val = value.as_u32() as u16;
        match offset {
            0x0 => {self.transfer = Some(val as u8); },
            0x8 => {self.mode.0 = val;},
            0xA => self.write_control(val),
            0xE => {self.boudrate_reload = val; },
            _   => panic!("Unhandled sio store{:?} offset: {:08x}", T::width(), offset)
        };
    }

    pub fn write_control(&mut self, val: u16) {
        self.control.0 = val & !0xC000;
        // println!("control: {:?}", self.control);

        if self.control.acknowledge() {
            self.status.set_interrupt_request(false);
            self.control.set_acknowledge(false);
        }

        if self.control.reset() {
            self.reset_regs();
        }

        if !self.control.dtr_output_level() {
            self.status.set_dsr_input_level(false);
        }

        if self.control.tx_enable() {
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
        self.gamepad.reset();
        self.memcard.reset();
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
                _    => (0xFF, State::None),

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
            let byte = self.gamepad.send_and_receive_byte(data);
            let state = if self.gamepad.in_ack() {
                State::GamepadComm
            } else {
                State::None
            };
            (byte, state)
        } else {
            (0xFF, State::None)
        }
        // (0xFF, State::None)
    }
    pub fn process_memcard(&mut self, port: usize, data: u8) -> (u8, State) {
        // if port == 0 {
        //     let byte = self.memcard.send_and_receive_byte(data);
        //     let state = if self.memcard.in_ack() {
        //         State::MemcardComm
        //     } else {
        //         State::None
        //     };
        //     (byte, state)
        // } else {
        //     (0xFF, State::None)
        // }
        (0xFF, State::None)
    }

    pub fn try_send_data(memory_bus: &mut MemoryBus) {
        if !memory_bus.sio.control.tx_enable() {
            return;
        }

        if let Some(val) = memory_bus.sio.transfer {
            // send/receive
            let (received, ack) = memory_bus.sio.send_and_receive_byte(val);
            
            let sio = &mut memory_bus.sio;
            sio.status.set_dsr_input_level(ack);

            if sio.control.dsr_interrupt_enable() && sio.status.dsr_input_level() {
                memory_bus.scheduler.schedule(crate::scheduler::Event::SerialSend, 
                    u64::from(sio.boudrate_reload) * 8, None);
            }

            if sio.status.dsr_input_level() {
                memory_bus.scheduler.schedule(crate::scheduler::Event::DsrOff, 64, None);
            }

            memory_bus.sio.push_received_data(received);

        }
    }

    pub fn turn_dsr_off(&mut self) {
        self.status.set_dsr_input_level(false);
    }

    pub fn process_serial_send(memory_bus: &mut MemoryBus) {
        memory_bus.irqctl.status.set_ctl_mem(true);
        memory_bus.sio.status.set_interrupt_request(true);
    }

    pub fn tick(memory_bus: &mut MemoryBus) {
        Self::try_send_data(memory_bus);
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
    #[derive(Default)]
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
    #[derive(Default)]
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
    #[derive(Default)]
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

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    None,
    GamepadComm,
    MemcardComm
}

pub struct Gamepad {
    state: GamepadState,
    mode: GamepadMode,
    in_ack: bool,
    pub digital_switches: u16,
    pub joystick_axes: [u8;4],
}

impl Gamepad {

    pub fn send_and_receive_byte(&mut self, data: u8) -> u8 {
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

        if let Some(state) = self.mode.next(self.state, data) {
            self.state = state;
            self.in_ack = state != GamepadState::Init;
            received
        } else {
            self.reset();
            0xFF
        }

    }

    pub fn in_ack(&self) -> bool {
        self.in_ack
    }

    pub fn reset(&mut self) {
        self.in_ack = false;
        self.state = GamepadState::Init;
    }

    pub fn set_stick_axis(&mut self, left: (u8, u8), right: (u8, u8)) {
        self.joystick_axes = [right.0, right.1, left.0, left.1];
    }

    pub fn set_buttons(&mut self, buttons: u16) {
        self.digital_switches = buttons;
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

#[derive(Default)]
struct Memcard {
}

impl Memcard {
    pub fn send_and_receive_byte(&mut self, data: u8) -> u8 {
       0
    }
    pub fn in_ack(&self) -> bool {
        false
    }

    pub fn reset(&mut self) {
    }
    
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
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
    AnalogInput3
}


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
    Triangle,
    Circle,
    Cross,
    Square,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum Axis {
    RightX,
    RightY,
    LeftX,
    LeftY,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum GamepadMode {
    #[default]
    Digital,
    Analog
}

impl GamepadMode {
    const DIGITAL_STATES: [(GamepadState, Option<u8>); 5] = [
        (GamepadState::Init, None),
        (GamepadState::IdLow, Some(0x01)),
        (GamepadState::IdHigh, Some(0x42)),
        (GamepadState::SwitchLow, None),
        (GamepadState::SwitchHigh, None)
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

    const fn id(self) -> [u8;2] {
        match self {
            Self::Digital => 0x5A41_u16.to_le_bytes(),
            Self::Analog =>  0x5A73_u16.to_le_bytes(),
        }
    }

    const fn state_table(self) -> &'static[(GamepadState, Option<u8>)] {
        match self {
            Self::Digital => &Self::DIGITAL_STATES,
            Self::Analog  => &Self::ANALOG_STATES,
        }
    }

    fn next(self, current_state: GamepadState, received_byte: u8) -> Option<GamepadState> {
        let idx = current_state as usize;
        let state_table = self.state_table();

        let (next_state, check_byte) = state_table[(idx + 1) % state_table.len()];
        let next_state_is_valid = check_byte.is_none_or(|b| b == received_byte);
        next_state_is_valid.then_some(next_state)
    }
}

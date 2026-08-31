use crate::{
    memory_bus::{Addressable, MemoryBus},
    scheduler::{LINE_DURATION, TimerInterrupt},
};

// almost verbatim copy of
// https://github.com/kaezrr/starpsx/blob/main/core/src/timers.rs

#[derive(Clone, Copy, PartialEq)]
pub enum SyncMode {
    FreeRun,
    Paused,
    ResetOnHSync,
    HSyncOnly,
    PauseOnHSync,
    ResetOnVSync,
    VSyncOnly,
    PauseOnVSync,
    StartOnNextLine,
    StartOnNextFrame,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ClockSource {
    Cpu,
    CpuDiv8,
    Dot,
    HBlank,
}

bitfield::bitfield! {
    #[derive(Default, Clone, Copy)]
    pub struct Mode(u32);
    sync_enabled, set_sync_enabled: 0;
    u8, sync_mode, _: 2,1;
    reset_to_target, _: 3; //  Reset counter to 0000h  (0=After Counter=FFFFh, 1=After Counter=Target)
    irq_target, _: 5,4;
    irq_repeat, _: 6;
    irq_toggle, _: 7;
    clock_src, _: 9,8;
    irq_disabled, set_irq_disabled: 10;
    reached_target, set_reached_target: 11;
    reached_end, set_reached_end: 12;

}
// 0     Synchronization Enable (0=Free Run, 1=Synchronize via Bit1-2)
// 1-2   Synchronization Mode   (0-3, see lists below)
//        Synchronization Modes for Counter 0:
//          0 = Pause counter during Hblank(s)
//          1 = Reset counter to 0000h at Hblank(s)
//          2 = Reset counter to 0000h at Hblank(s) and pause outside of Hblank
//          3 = Pause until Hblank occurs once, then switch to Free Run
//        Synchronization Modes for Counter 1:
//          Same as above, but using Vblank instead of Hblank
//        Synchronization Modes for Counter 2:
//          0 or 3 = Stop counter at current value (forever, no h/v-blank start)
//          1 or 2 = Free Run (same as when Synchronization Disabled)
// 3     Reset counter to 0000h  (0=After Counter=FFFFh, 1=After Counter=Target)
// 4     IRQ when Counter=Target (0=Disable, 1=Enable)
// 5     IRQ when Counter=FFFFh  (0=Disable, 1=Enable)
// 6     IRQ Once/Repeat Mode    (0=One-shot, 1=Repeatedly)
// 7     IRQ Pulse/Toggle Mode   (0=Short Bit10=0 Pulse, 1=Toggle Bit10 on/off)
// 8-9   Clock Source (0-3, see list below)
//        Counter 0:  0 or 2 = System Clock,  1 or 3 = Dotclock
//        Counter 1:  0 or 2 = System Clock,  1 or 3 = Hblank
//        Counter 2:  0 or 1 = System Clock,  2 or 3 = System Clock/8
// 10    Interrupt Request       (0=Yes, 1=No) (Set after Writing)    (W=1) (R)
// 11    Reached Target Value    (0=No, 1=Yes) (Reset after Reading)        (R)
// 12    Reached FFFFh Value     (0=No, 1=Yes) (Reset after Reading)        (R)
// 13-15 Unknown (seems to be always zero)
// 16-31 Garbage (next opcode)

#[derive(Default)]
pub struct Timer {
    counter: u32,
    mode: Mode,
    target: u16,
    last_read: u64
}

impl Timer {
    pub fn set_mode(&mut self, v: u32) {
        // Bits 12-11 are read only.
        self.mode.0 = (v & !0x1800) | (self.mode.0 & 0x1800);
        self.counter = 0;
        self.mode.set_irq_disabled(true);
    }

    pub fn get_mode(&mut self) -> u32 {
        let r = self.mode.0;
        self.mode.set_reached_end(false);
        self.mode.set_reached_target(false);
        r
    }


    pub fn get_ticks_to_value(&self, target: u16) -> u32 {
        let counter = self.counter;
        let target = u32::from(target);
        let reset = if self.mode.reset_to_target() {
            target
        } else {
            0xFFFF
        };
        let period = reset + 1;
        match counter.cmp(&target) {
            std::cmp::Ordering::Less => target - counter,
            std::cmp::Ordering::Equal => period,
            std::cmp::Ordering::Greater => period - counter + target,
        }
    }
}

pub struct Timers {
    timers: [Timer; 3],
    in_hsync: bool,
    in_vsync: bool,
    hblanks: u32,
}

impl Timers {
    pub fn new() -> Self {
        Timers {
            timers: [Timer::default(), Timer::default(), Timer::default()],
            in_hsync: false,
            in_vsync: false,
            hblanks: 0,
        }
    }
    pub fn sync_mode(&self, which: usize) -> SyncMode {
        let timer = &self.timers[which];
        if timer.mode.sync_enabled() {
            let sync_raw = timer.mode.sync_mode();
            SYNC_MODE_MATRIX[which][sync_raw as usize]

        } else {
           SyncMode::FreeRun
        }
    }

    pub fn clock_source(&self, which: usize) -> ClockSource {
        let source_raw = self.timers[which].mode.clock_src();
        CLOCK_SOURCE_MATRIX[which][source_raw as usize]
    }

    pub fn ticks_to_cycles(memory_bus: &MemoryBus, which: usize, ticks: u32) -> u64 {
        let ticks = ticks as u64;
        match memory_bus.timers.clock_source(which) {
            ClockSource::Cpu => ticks,
            ClockSource::CpuDiv8 => ticks * 8,
            ClockSource::Dot => ticks * 5, // FIXME: (memory_bus.gpu.get_clock_divider() as u64),
            ClockSource::HBlank => ticks * LINE_DURATION,
        }
    }

    pub fn enter_vsync(memory_bus: &mut MemoryBus) {
        Self::update_value(memory_bus, 1);
        memory_bus.timers.in_vsync = true;
        match memory_bus.timers.sync_mode(1) {
            SyncMode::ResetOnVSync | SyncMode::VSyncOnly => memory_bus.timers.timers[1].counter = 0,
            _ => (),
        }
    }

    pub fn exit_vsync(memory_bus: &mut MemoryBus) {
        memory_bus.timers.in_vsync = false;
        if memory_bus.timers.sync_mode(1) == SyncMode::StartOnNextFrame {
            memory_bus.timers.timers[1].mode.set_sync_enabled(false)
        }
    }

    pub fn enter_hsync(memory_bus: &mut MemoryBus) {
        Self::update_value(memory_bus, 0);
        memory_bus.timers.hblanks += 1;
        memory_bus.timers.in_hsync = true;
        match memory_bus.timers.sync_mode(0) {
            SyncMode::ResetOnHSync | SyncMode::HSyncOnly => memory_bus.timers.timers[0].counter = 0,
            _ => (),
        }
    }

    pub fn exit_hsync(memory_bus: &mut MemoryBus) {
        memory_bus.timers.in_hsync = false;
        if memory_bus.timers.sync_mode(0) == SyncMode::StartOnNextLine {
            memory_bus.timers.timers[0].mode.set_sync_enabled(false)
        }
        memory_bus.timers.timers[0].last_read = memory_bus.scheduler.cycle;
        Self::reschedule_interrupt_if_needed(memory_bus, 0);
    }



    fn update_value(memory_bus: &mut MemoryBus, which: usize) {
        let timer =  &mut memory_bus.timers.timers[which];

        let clock_delta = (memory_bus.scheduler.cycle - timer.last_read) as u32;
        timer.last_read = memory_bus.scheduler.cycle;

        match memory_bus.timers.sync_mode(which) {
            SyncMode::Paused => return,
            SyncMode::PauseOnHSync if memory_bus.timers.in_hsync => return,
            SyncMode::PauseOnVSync if memory_bus.timers.in_vsync => return,
            SyncMode::HSyncOnly if !memory_bus.timers.in_hsync => return,
            SyncMode::VSyncOnly if !memory_bus.timers.in_vsync => return,
            _ => (),
        }

        let timer =  &mut memory_bus.timers.timers[which];

        let reset = if timer.mode.reset_to_target() {
            u32::from(timer.target)
        } else {
            0xFFFF
        };

        // Only drain hblanks when this timer actually uses the HBlank clock.
        let delta = match memory_bus.timers.clock_source(which) {
            ClockSource::Cpu => clock_delta,
            ClockSource::CpuDiv8 => clock_delta / 8,
            ClockSource::Dot => clock_delta / 5, // FIXME: (memory_bus.gpu.get_clock_divider() as u32),
            ClockSource::HBlank => {
                let h = memory_bus.timers.hblanks;
                memory_bus.timers.hblanks = 0;
                h
            }
        };

        let timer =  &mut memory_bus.timers.timers[which];
        let old_counter = timer.counter as u64;
        let delta64 = delta as u64;
        let target64 = timer.target as u64;
        let ffff64 = 0xFFFFu64;
        let reset64 = reset as u64;

        timer.mode.set_reached_target(delta64 > 0 && old_counter + delta64 > target64);

        let crossed_ffff = if timer.mode.reset_to_target() {
            false
        } else {
            delta64 > 0 && old_counter + delta64 > ffff64
        };
        timer.mode.set_reached_end(crossed_ffff);

        timer.counter = ((old_counter + delta64) % (reset64 + 1)) as u32;
    }

    fn reschedule_interrupt_if_needed(memory_bus: &mut MemoryBus, which: usize) {
        match memory_bus.timers.sync_mode(which) {
            SyncMode::Paused => return,
            SyncMode::PauseOnHSync if memory_bus.timers.in_hsync => return,
            SyncMode::PauseOnVSync if memory_bus.timers.in_vsync => return,
            SyncMode::HSyncOnly if !memory_bus.timers.in_hsync => return,
            SyncMode::VSyncOnly if !memory_bus.timers.in_vsync => return,
            _ => (),
        }

        let timer = &memory_bus.timers.timers[which];
        let ticks_till_target = timer.get_ticks_to_value(timer.target);
        let ticks_till_ffff = timer.get_ticks_to_value(0xFFFF);
        let target = timer.target as u32;

        let cycles_till_target = Self::ticks_to_cycles(memory_bus, which, ticks_till_target);
        let cycles_till_target_reset = Self::ticks_to_cycles(memory_bus, which, target);


        let cycles_till_ffff = Self::ticks_to_cycles(memory_bus, which, ticks_till_ffff);
        let cycles_till_ffff_reset = Self::ticks_to_cycles(memory_bus, which, 0xFFFF);

        let (cycles_till_irq, cycles_till_irq_reset) = match timer.mode.irq_target() {
            // no irq
            0 => return,
            1 => (cycles_till_target, cycles_till_target_reset),
            2 => (cycles_till_ffff, cycles_till_ffff_reset),
            //  Both FFFF and Target IRQ — schedule whichever comes first
            3 => {
                if cycles_till_target < cycles_till_ffff {
                    (cycles_till_target, cycles_till_target_reset)
                } else {
                    (cycles_till_ffff, cycles_till_ffff_reset)
                }
            },
            _ => unreachable!()
        };

        memory_bus.scheduler.schedule(
            crate::scheduler::Event::Timer(TimerInterrupt {
                which, toggle: timer.mode.irq_toggle()
            }),
            cycles_till_irq,
            timer.mode.irq_repeat().then_some(cycles_till_irq_reset),
        );


    }

    pub fn process_interrupt(memory_bus: &mut MemoryBus, irq: TimerInterrupt) {
        let timer = &mut memory_bus.timers.timers[irq.which];
        let set_irq = if irq.toggle {
            let prev = timer.mode.irq_disabled();
            let next = !prev;
            timer.mode.set_irq_disabled(next);
            prev && !next // FIXME: woot?
        } else {
            timer.mode.set_irq_disabled(true);
            true
        };

        if set_irq {
            match irq.which {
                0 => memory_bus.irqctl.status.set_tmr0(true),
                1 => memory_bus.irqctl.status.set_tmr1(true),
                2 => memory_bus.irqctl.status.set_tmr2(true),
                _ => unreachable!("there're only three timers")
            }
        }

    }
}

pub fn load<T: Addressable>(memory_bus: &mut MemoryBus, offset: u32) -> T {
    let which = (offset >> 4) as usize;
    Timers::update_value(memory_bus, which);

    let timer = &mut memory_bus.timers.timers[which];
    let v = match offset & 0xF {
        0 => timer.counter,
        4 => timer.get_mode(),
        8 => timer.target as u32,
        _ => panic!("timer load for offset 0x{} unimplemented", offset),
    };
    T::from_u32(v)
}

pub fn store<T: Addressable>(memory_bus: &mut MemoryBus, offset: u32, v: T) {
    let which = (offset >> 4) as usize;
    Timers::update_value(memory_bus, which);
    let timer = &mut memory_bus.timers.timers[which];
    match offset & 0xF {
        0 => timer.counter = v.as_u32(),
        4 => timer.set_mode(v.as_u32()),
        8 => timer.target = v.as_u32() as u16,
        _ => panic!("timer store for offset 0x{} unimplemented", offset),
    };
    Timers::reschedule_interrupt_if_needed(memory_bus, which);
}

const CLOCK_SOURCE_MATRIX: [[ClockSource; 4]; 3] = [
    [
        ClockSource::Cpu,
        ClockSource::Dot,
        ClockSource::Cpu,
        ClockSource::Dot,
    ],
    [
        ClockSource::Cpu,
        ClockSource::HBlank,
        ClockSource::Cpu,
        ClockSource::HBlank,
    ],
    [
        ClockSource::Cpu,
        ClockSource::Cpu,
        ClockSource::CpuDiv8,
        ClockSource::CpuDiv8,
    ],
];

const SYNC_MODE_MATRIX: [[SyncMode; 4]; 3] = [
    [
        SyncMode::PauseOnHSync,    //  Pause counter during Hblank(s)
        SyncMode::ResetOnHSync,    //  Reset counter to 0000h at Hblank(s)
        SyncMode::HSyncOnly, // Reset counter to 0000h at Hblank(s) and pause outside of Hblank
        SyncMode::StartOnNextLine, // Pause until Hblank occurs once, then switch to Free Run
    ],
    [
        SyncMode::PauseOnVSync, // same as above only for VSync
        SyncMode::ResetOnVSync,
        SyncMode::VSyncOnly,
        SyncMode::StartOnNextFrame,
    ],
    [
        SyncMode::Paused,  // Stop counter at current value
        SyncMode::FreeRun, // same as when sync disabled
        SyncMode::FreeRun,
        SyncMode::Paused,
    ],
];

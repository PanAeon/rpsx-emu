use arrayvec::ArrayVec;

pub const LINE_DURATION: u64 = 2172;
pub const HBLANK_DURATION: u64 = 390;
pub const SPU_INTERVAL: u64 = 668;
pub const SAMPLES_PER_FRAME: u64 = 735;
pub const CLOCKS_PER_FRAME: u64 = SAMPLES_PER_FRAME * SPU_INTERVAL;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TimerInterrupt {
    pub which: usize,
    pub toggle: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Event {
    VBlankStart,
    VBlankEnd,
    HBlankStart,
    HBlankEnd,
    SpuTick,
    Timer(TimerInterrupt),
    CDRom(u8, [u8;16],usize)
}

pub struct Task {
    cycle: u64,
    event: Event,
    repeat: Option<u64>
}

#[derive(Default)]
pub struct Scheduler {
    tasks: ArrayVec<Task, 32>,
    pub cycle: u64,
}
// FIXME: we have a problem: 668*735/2172 = 226.049.... gpu is not in sync
impl Scheduler {
    // pub fn new() -> Self {
    //     let mut timeline = vec![];
    //     for i in 1..=SAMPLES_PER_FRAME {
    //         timeline.push((Event::SpuTick, i*SPU_INTERVAL));
    //     }
    //     for i in 1..=240 {
    //         timeline.push((Event::HBlankStart,i*LINE_DURATION - HBLANK_DURATION));
    //         timeline.push((Event::HBlankEnd,i*LINE_DURATION));
    //     }
    //     timeline.push((Event::VBlankStart, 240*LINE_DURATION));
    //     timeline.push((Event::VBlankEnd, 263*LINE_DURATION));
    //     timeline.sort_by(|a,b| a.1.cmp(&b.1));
    //     Scheduler {
    //         timeline,
    //         cycle: 0,
    //         current: 0
    //     }
    //
    // }
    pub fn get_next_event(&mut self) -> Option<Event> {
        if self.cycle < self.tasks.first()?.cycle {
            return None;
        }
        let task = self.tasks.remove(0);

        if let Some(cycles) = task.repeat {
            self.schedule(task.event.clone(), cycles, Some(cycles))
        }

        Some(task.event)
    }

    pub fn unschedule(&mut self, event: &Event) {
        self.tasks.retain(|x| x.event != *event);
    }

    pub fn schedule(&mut self, event: Event, cycles: u64, repeat: Option<u64>) {
        self.unschedule(&event);

        let cycle = self.cycle + cycles;

        // TODO: check that this is correct
        let pos = self.tasks.iter().position(|e| e.cycle > cycle)
            .unwrap_or(self.tasks.len());
        self.tasks.insert(pos, Task { cycle, event, repeat });
    }

    pub fn advance(&mut self, num_cycles: u64) {
        self.cycle += num_cycles;
    }

    pub fn init(&mut self) {
        self.schedule(Event::VBlankStart, LINE_DURATION * 240, Some(LINE_DURATION * 263));
        self.schedule(Event::VBlankEnd, LINE_DURATION * 263, Some(LINE_DURATION * 263));
        self.schedule(Event::HBlankStart, LINE_DURATION - HBLANK_DURATION, Some(LINE_DURATION));
        self.schedule(Event::HBlankEnd, LINE_DURATION, Some(LINE_DURATION));
        self.schedule(Event::SpuTick, 768, Some(768));
    }

}

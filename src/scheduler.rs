use arrayvec::ArrayVec;

use crate::{ gpu::VideoStandard, scheduler::Event::CDRomResultIrq};
use serde::{Deserialize, Serialize};

pub const NTSC_LINE_DURATION: u64 = 2172; // in cpu cycles NTSC, 263 scanlines per frame
pub const PAL_LINE_DURATION: u64 = 2153; // in cpu cycles NTSC, 263 scanlines per frame
pub const NTSC_HBLANK_DURATION: u64 = 538;
pub const PAL_HBLANK_DURATION: u64 = 538;
// pub const NTSC_HBLANK_DURATION: u64 = 390;
// pub const PAL_HBLANK_DURATION: u64 = 390;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TimerInterrupt {
    pub which: usize,
    pub toggle: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    VBlankStart,
    VBlankEnd,
    HBlankStart,
    HBlankEnd,
    SpuTick,
    Timer(TimerInterrupt),
    CDRomResultIrq(u8),
    SerialSend,
    DsrOff,
    Dma,
    LightGun(u16, u16, u16),
}

impl Event {
    pub fn is_cdrom(&self) -> bool {
        match self {
            CDRomResultIrq(_) => true,
            _ => false
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Task {
    cycle: u64,
    event: Event,
    repeat: Option<u64>
}

#[derive(Default, Serialize, Deserialize)]
pub struct Scheduler {
    tasks: ArrayVec<Task, 32>,
    pub cycle: u64,
}
//  we have a problem: 668*735/2172 = 226.049.... gpu is not in sync
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

    // pub fn next_event_budget(&mut self) -> u64 {
    //     match self.tasks.first() {
    //         Some(task) => task.cycle - self.cycle,
    //         None => 1024
    //     }
    // }

    pub fn unschedule_light_gun(&mut self) {
        self.tasks.retain(|x| match x.event {
            Event::LightGun(_,_ ,_) => false,
            _ => true
        });
    }
    pub fn unschedule(&mut self, event: &Event) {
        self.tasks.retain(|x| x.event != *event);
    }
    // pub fn unschedule2(&mut self, event: &Event) {
    //     self.tasks.retain(|x| x.event != *event || x.event.is_cdrom());
    // }

    pub fn schedule(&mut self, event: Event, cycles: u64, repeat: Option<u64>) {
        self.unschedule(&event);

        let cycle = self.cycle + cycles;

        let pos = self.tasks.iter().position(|e| e.cycle > cycle)
            .unwrap_or(self.tasks.len());
        self.tasks.insert(pos, Task { cycle, event, repeat });
    }

    pub fn advance(&mut self, num_cycles: u64) {
        self.cycle += num_cycles;
    }

    pub fn init(&mut self, video_standard: VideoStandard) {
        let line_duration = match video_standard {
            VideoStandard::NTSC => NTSC_LINE_DURATION,
            VideoStandard::PAL => PAL_LINE_DURATION,
        };
        let hblank_duration = match video_standard {
            VideoStandard::NTSC => NTSC_HBLANK_DURATION,
            VideoStandard::PAL => PAL_HBLANK_DURATION,
        };

        let total_lines = match video_standard {
            VideoStandard::NTSC => 263,
            VideoStandard::PAL => 314,
        };
        let display_lines = match video_standard {
            VideoStandard::NTSC => 240,
            VideoStandard::PAL => 256,// approx.
        };
        self.schedule(Event::VBlankStart, line_duration * display_lines, Some(line_duration * total_lines));
        self.schedule(Event::VBlankEnd, line_duration * total_lines, Some(line_duration * total_lines));
        self.schedule(Event::HBlankStart, line_duration - hblank_duration, Some(line_duration));
        self.schedule(Event::HBlankEnd, line_duration, Some(line_duration));
        self.schedule(Event::SpuTick, 768, Some(768));
        // self.schedule(Event::Dma, 128, Some(128));
    }

}

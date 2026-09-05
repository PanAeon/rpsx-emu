use crate::system::{AccessWidth, Addressable};


bitfield::bitfield! {
    #[derive(Default, Clone, Copy)]
    pub struct InterruptStatus(u32);
    pub _, set_vblank: 0;
    pub _, set_gpu: 1;
    pub _, set_cdrom: 2;
    pub _, set_dma: 3;
    pub _, set_tmr0: 4;
    pub _, set_tmr1: 5;
    pub _, set_tmr2: 6;
    pub _, set_ctl_mem: 7;
    pub _, set_sio: 8;
    pub _, set_spu: 9;
    pub _, set_ctl_light: 10;
}
  // 0     IRQ0 VBLANK (PAL=50Hz, NTSC=60Hz)
  // 1     IRQ1 GPU   Can be requested via GP0(1Fh) command (rarely used)
  // 2     IRQ2 CDROM
  // 3     IRQ3 DMA
  // 4     IRQ4 TMR0  Timer 0 aka Root Counter 0 (Sysclk or Dotclk)
  // 5     IRQ5 TMR1  Timer 1 aka Root Counter 1 (Sysclk or H-blank)
  // 6     IRQ6 TMR2  Timer 2 aka Root Counter 2 (Sysclk or Sysclk/8)
  // 7     IRQ7 Controller and Memory Card - Byte Received Interrupt
  // 8     IRQ8 SIO
  // 9     IRQ9 SPU
  // 10    IRQ10 Controller - Lightpen Interrupt. Also shared by PIO and DTL cards.
  // 11-15 Not used (always zero)
  // 16-31 Garbage


#[derive(Default)]
pub struct InterruptController {
    pub status: InterruptStatus, // R - status, W - acknowledge
    mask: u32,
}

impl InterruptController {
    pub fn pending(&self) -> bool {
        self.status.0 & self.mask != 0
    }
    pub fn load<T:Addressable>(&self, offset: u32) -> T {
        // if T::width() != AccessWidth::Word {
        //     panic!("irqctl load for {:?} not implemented", T::width());
        // }
        match offset {
            0 => T::from_u32(self.status.0),
            4 => T::from_u32(self.mask),
            _ => panic!("irqctl load offset {} not implemented", offset),
        }
    }
    pub fn store<T:Addressable>(&mut self, offset: u32, value: T) {
        // if T::width() != AccessWidth::Word {
        //     panic!("irqctl store for {:?} not implemented", T::width());
        // }
        match offset {
            0 => self.status.0 &= value.as_u32(),
            4 => self.mask = value.as_u32(),
            _ => panic!("irqctl store offset {} not implemented", offset),
        }
    }
}

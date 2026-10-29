use crate::system::{AccessWidth, Addressable};
use serde::{Deserialize, Serialize};


bitfield::bitfield! {
    #[derive(Default, Clone, Copy, Serialize, Deserialize)]
    pub struct InterruptStatus(u32);
    pub vblank, set_vblank: 0;
    pub gpu, set_gpu: 1;
    pub cdrom, set_cdrom: 2;
    pub dma, set_dma: 3;
    pub tmr0, set_tmr0: 4;
    pub tmr1, set_tmr1: 5;
    pub tmr2, set_tmr2: 6;
    pub ctl_mem, set_ctl_mem: 7;
    pub sio, set_sio: 8;
    pub spu, set_spu: 9;
    pub ctl_light, set_ctl_light: 10;
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


#[derive(Default, Serialize, Deserialize)]
pub struct InterruptController {
    status: InterruptStatus, // R - status, W - acknowledge
    int: InterruptStatus, // R - status, W - acknowledge
    mask: u32,
}

impl InterruptController {
    pub fn pending(&self) -> bool {
        self.int.0 & self.mask != 0
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
            0 => {
                self.status.0 &= value.as_u32();
                self.int.0 &= value.as_u32();
            },
            4 => self.mask = value.as_u32(),
            _ => panic!("irqctl store offset {} not implemented", offset),
        }
    }

    pub fn set_vblank(&mut self) {
         if !self.status.vblank() {
            self.int.set_vblank(true);
            self.status.set_vblank(true);
         }
    }

    pub fn set_dma(&mut self) {
         if !self.status.dma() {
            self.int.set_dma(true);
            self.status.set_dma(true);
         }
    }
    pub fn set_spu(&mut self) {
         if !self.status.spu() {
            self.int.set_spu(true);
            self.status.set_spu(true);
         }
    }
    pub fn set_cdrom(&mut self) {
         if !self.status.cdrom() {
            self.int.set_cdrom(true);
            self.status.set_cdrom(true);
         }
    }
    pub fn set_tmr0(&mut self) {
         if !self.status.tmr0() {
            self.int.set_tmr0(true);
            self.status.set_tmr0(true);
         }
    }

    pub fn set_tmr1(&mut self) {
         if !self.status.tmr1() {
            self.int.set_tmr1(true);
            self.status.set_tmr1(true);
         }
    }

    pub fn set_tmr2(&mut self) {
         if !self.status.tmr2() {
            self.int.set_tmr2(true);
            self.status.set_tmr2(true);
         }
    }

    pub fn set_ctl_mem(&mut self) {
         if !self.status.ctl_mem() {
            self.int.set_ctl_mem(true);
            self.status.set_ctl_mem(true);
         }
    }


}

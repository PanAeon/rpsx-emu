
#[derive(Default)]
pub struct Gte {
    // control registers

  // cop2r32-36 9xS16 RT11RT12,..,RT33 Rotation matrix     (3x3)        ;cnt0-4
  // cop2r37-39 3x 32 TRX,TRY,TRZ      Translation vector  (X,Y,Z)      ;cnt5-7
  // cop2r40-44 9xS16 L11L12,..,L33    Light source matrix (3x3)        ;cnt8-12
  // cop2r45-47 3x 32 RBK,GBK,BBK      Background color    (R,G,B)      ;cnt13-15
  // cop2r48-52 9xS16 LR1LR2,..,LB3    Light color matrix source (3x3)  ;cnt16-20
  // cop2r53-55 3x 32 RFC,GFC,BFC      Far color           (R,G,B)      ;cnt21-23
  // cop2r56-57 2x 32 OFX,OFY          Screen offset       (X,Y)        ;cnt24-25
  // cop2r58 BuggyU16 H                Projection plane distance.       ;cnt26
  // cop2r59      S16 DQA              Depth queing parameter A (coeff) ;cnt27
  // cop2r60       32 DQB              Depth queing parameter B (offset);cnt28
  // cop2r61-62 2xS16 ZSF3,ZSF4        Average Z scale factors          ;cnt29-30
  // cop2r63      U20 FLAG             Returns any calculation errors   ;cnt31
  
  pub r56: u32,
  pub r57: u32,
  pub r58: u16,
  pub r59: i16,
  pub r60: u32,
  pub r61: i16, // cnt2929
  pub r62: i16, // cnt30
  

}

impl Gte {
}

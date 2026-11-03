use vek::num_integer::Integer;


pub const POS_ADPCM_TABLE: [i32; 5] = [0, 60, 115, 98, 122];
pub const NEG_ADPCM_TABLE: [i32; 5] = [0, 0, -52, -55, -60];

// returns shift, pos_filter, neg_filter
pub fn decode_params(header: u8, max_filter: u8) -> (u8, i32, i32){
    let mut shift = header & 0x0f;
    let mut filter = (header & 0x70) >> 4;

    if shift > 12 {
        shift = 9;
    }

    if filter > max_filter {
        filter = max_filter;
    }

    (shift, POS_ADPCM_TABLE[filter as usize], NEG_ADPCM_TABLE[filter as usize])



}

pub fn decode_xa_block(
   header: u8,
   data: &[u8],
   nibble_shift: u8,
   prev_samples: &mut [i32;2],
   out: &mut[i16;28]) {
    assert!(data.len() >= 28);

    let (shift, filter_pos, filter_neg) = decode_params(header, 3);

    for n in 0..28 {
        let raw_nibble = ((data[n] >> nibble_shift) & 0x0f) as i16; // u4
        // sign extend shift = 16-4
        let nibble: i16 = (raw_nibble << 12) >> 12;

        let mut sample: i32 = (nibble as i32) << 12;
        sample >>= shift;
        sample += 
            vek::num_integer::div_floor(prev_samples[0] * filter_pos + prev_samples[1] * filter_neg + 32,64);
            

        out[n] = sample.clamp(-0x8000, 0x7fff) as i16;
        prev_samples[1] = prev_samples[0];
        prev_samples[0] = sample;
    }
}

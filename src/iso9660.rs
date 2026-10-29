// from https://github.com/simias/rustation-ng/blob/main/src/psx/cd/iso9660.rs
//! Very partial implementation of the ISO 9660 filesystem

use std::sync::Arc;

use crate::{cd_cache::CdCache, cdrom::SECTOR_SIZE};
use cdimage::sector::Sector;

/// Structure representing an ISO9660 directory
pub struct Directory {
    /// Contents of the directory
    pub entries: Vec<Entry>,
}

impl Directory {
    pub fn new(image: &mut CdCache, entry: &Entry) -> Result<Directory, IsoError> {
        if !entry.is_dir() {
            return Err(IsoError::NotADirectory);
        }

        let mut dir = Directory {
            entries: Vec::new(),
        };

        // Directory entries cannot span multiple sectors so it's safe to handle them one by one
        let mut extent_len = entry.extent_len() as usize;
        let extent_location = entry.extent_location(); // our sector index..
        //
        // println!("extent len: {}", extent_len);
        // println!("extent loc: {}", extent_location);

        // let track_msf = match Msf::from_sector_index(extent_location) {
        //     Some(m) => m,
        //     None => return Err(IsoError::BadExtent(extent_location)),
        // };

        // let toc = image.toc();
        // let track = toc.track(Bcd::ONE)?;
        // TODO: maybe need to add  lead-in? not sure
        let mut dp = extent_location as usize * SECTOR_SIZE; //track.disc_position(track_msf)?;

        while extent_len > 0 {
            let sector = image.read_sector(dp).expect("ok");

            let data = mode2_xa_payload(&sector);

            let len = if extent_len > 2048 { 2048 } else { extent_len };

            dir.parse_entries(&data[0..len])?;

            extent_len -= len;
            dp += SECTOR_SIZE;
        }

        Ok(dir)
    }

    pub fn parse_entries(&mut self, mut raw: &[u8]) -> Result<(), IsoError> {
        while !raw.is_empty() {
            let dir_len = raw[0] as usize;

            // println!("dir len: {}", dir_len);

            if dir_len == 0 {
                // It seems we've reached the last directory. Or at least I think so? I'm not
                // entirely sure how directories which span several sectors are handled, if the
                // padding is not part of any entry then we should skip ahead to the next sector.
                // Needs more testing.
                break;
            }

            if dir_len < 34 {
                return Err(IsoError::InvalidDirectoryEntryLen(dir_len));
            }

            let name_len = raw[32] as usize;

            let name_end = 33 + name_len;

            if name_end > dir_len {
                return Err(IsoError::EntryNameTooLong);
            }

            self.entries.push(Entry::new(&raw[0..dir_len]));

            raw = &raw[dir_len..];
        }

        Ok(())
    }

    /// Attempt to "cd" to a subdirectory, returning a new `Directory`
    /// instance
    #[allow(dead_code)]
    pub fn cd(&self, image: &mut CdCache, name: &[u8]) -> Result<Directory, IsoError> {
        let entry = self.entry_by_name(name)?;

        Directory::new(image, entry)
    }

    pub fn entry_by_name(&self, name: &[u8]) -> Result<&Entry, IsoError> {
        match self.entries.iter().find(|e| e.name() == name) {
            Some(e) => Ok(e),
            None => {
                let n = String::from_utf8_lossy(name);
                Err(IsoError::EntryNotFound(n.to_string()))
            }
        }
    }

    /// Retreive a list of all the entries in this directory
    #[allow(dead_code)]
    pub fn ls(&self) -> &[Entry] {
        &self.entries
    }
}

/// A single directory entry
pub struct Entry(Vec<u8>);

impl Entry {
    fn new(entry: &[u8]) -> Entry {
        Entry(entry.into())
    }

    pub fn name(&self) -> &[u8] {
        let name_len = self.0[32] as usize;

        let name_end = 33 + name_len;

        // No need to validate the len, it should've been done on
        // entry creation
        &self.0[33..name_end]
    }

    pub fn is_dir(&self) -> bool {
        let flags = self.0[25];

        (flags & 0x2) != 0
    }

    pub fn extent_location(&self) -> u32 {
        read_u32(&self.0[2..10])
    }

    pub fn extent_len(&self) -> u32 {
        read_u32(&self.0[10..18])
    }

    pub fn read_file(&self, image: &mut CdCache) -> Result<Vec<u8>, IsoError> {
        if self.is_dir() {
            return Err(IsoError::NotAFile);
        }

        let mut extent_len = self.extent_len() as usize;
        let extent_location = self.extent_location();

        let mut contents = Vec::with_capacity(extent_len);

        // let track_msf = match Msf::from_sector_index(extent_location) {
        //     Some(m) => m,
        //     None => return Err(IsoError::BadExtent(extent_location)),
        // };

        // let toc = image.toc();
        // let track = toc.track(Bcd::ONE)?;
        let mut dp = extent_location as usize * SECTOR_SIZE; //track.disc_position(track_msf)?;

        while extent_len > 0 {
            let sector = image.read_sector(dp).expect("ok");

            let data = mode2_xa_payload(&sector);

            let len = if extent_len > 2048 { 2048 } else { extent_len };

            for &b in &data[0..len] {
                contents.push(b);
            }

            extent_len -= len;
            dp += SECTOR_SIZE;
        }

        Ok(contents)
    }
}

#[derive(Debug)]
pub enum IsoError {
    CdError,
    CachedCdError,
    BadMagic,
    MissingPrimaryVolumeDescriptor,
    BadVolumDescriptorVersion,
    BadExtent(u32),
    InvalidDirectoryEntryLen(usize),
    EntryNameTooLong,
    EntryNotFound(String),
    NotADirectory,
    NotAFile,
}

pub fn open_image(image: &mut CdCache) -> Directory {
    // The first 16 sectors are the "system area" which is ignored by the ISO filesystem. The
    // Volume Descriptor Set should start at 00:00:16 in track 01
    // let track = toc.track(Bcd::ONE)?;
    // we have just one track... btw...
    // let mut dp = 0;
    let mut dp = 16 * SECTOR_SIZE;// ignore lead-in
    // let mut dp = track.disc_position(Msf::from_bcd(0, 0, 0x16).unwrap())?;

    let mut sector;

    // Look for the primary volume descriptor
    loop {
        sector = image.read_sector(dp).expect("ok");

        let volume_descriptor = mode2_xa_payload(&sector);

        // Check volume descriptor "standard identifier"
        if &volume_descriptor[1..6] != b"CD001" {
            panic!("bad magic");
        }

        // Byte 0 contains the "volume descriptor type".
        match volume_descriptor[0] {
            // Primary Volume Descriptor
            0x01 => break,
            // Volume Descriptor Set Terminator
            0xff => panic!("missing volume descriptor"),
            // Unhandled volume descriptor type, ignore
            _ => (),
        }

        // Not the primary volume descriptor, move on to the next sector
        dp += SECTOR_SIZE ;
    }

    let volume_descriptor = mode2_xa_payload(&sector);

    // Volume Descriptor Version
    if volume_descriptor[6] != 0x01 {
        panic!("bad volume descr version")
    }

    // We can now open the root directory descriptor
    let root_dir_descriptor = &volume_descriptor[156..190];

    let root_dir = Entry::new(root_dir_descriptor);
    Directory::new(image, &root_dir).expect("OK")
}

/// Read a 32bit number stored in "both byte order" format
fn read_u32(v: &[u8]) -> u32 {
    // Only use the little endian representation. Should we bother validating that the BE version
    // is coherent?
    v[0] as u32 | ((v[1] as u32) << 8) | ((v[2] as u32) << 16) | ((v[3] as u32) << 24)
}

fn mode2_xa_payload(sector_data: &[u8]) -> Vec<u8> {
    let subheader = XaSubHeader(mode2_xa_subheader(sector_data));
    let payload = match subheader.submode().form() {
            XaForm::Form1 => sector_data[24..2072].to_vec(),
            XaForm::Form2 => sector_data[24..2348].to_vec(),
    };
    payload
    // sector_data[24..2072].to_vec()
    // let mut v = sector_data.to_vec();
                // v.drain(..0xC);
    // v.drain(0x818..);
    // v.drain(..0x18);
    
}
    /// Retrieve the CD-ROM XA Mode2 subheader. Returns `CdError::BadFormat` if this is not a
    /// CD-ROM XA Mode 2 sector.
    pub fn mode2_xa_subheader(sector_data: &[u8]) -> Vec<u8>{
        sector_data[16..16+8].to_vec()
    }

    /// Retrieve a CD-ROM XA Mode 2 payload. Returns `CdError::BadFormat` if this is not a Mode 2
    /// sector.
    ///
    /// For Mode 2 tracks the slice returned will be either be 2048 or 2324 bytes long depending on
    /// whether the sector is form 1 or form 2 respectively.
    // pub fn mode2_xa_payload(&self) -> CdResult<&[u8]> {
    //     let subheader = self.mode2_xa_subheader()?;
    //
    //     let payload = match subheader.submode().form() {
    //         XaForm::Form1 => &self.data[24..2072],
    //         XaForm::Form2 => &self.data[24..2348],
    //     };
    //
    //     Ok(payload)
    // }
    /// Returns the raw 16bit CD-ROM header for this sector. Returns an error if this is not a
    /// CD-ROM track (per sub-Q). If the header wasn't available in the original image format, it
    /// will be created on the fly.

    // Parse the CD-ROM header and return it. Same failure mode as `Sector::cd_rom_header_raw` but
    // will also fail if the sync pattern or header format is incorrect.
    /*pub fn cdrom_header(&self) -> CdResult<CdRomHeader> {
        let header = self.cd_rom_header_raw()?;

        // Validate sync pattern
        if header[0] != 0 || header[11] != 0 {
            return Err(CdError::BadSyncPattern);
        }

        if header.iter().take(11).skip(1).any(|&b| b != 0xff) {
            return Err(CdError::BadSyncPattern);
        }

        let m = if self.q.is_lead_in() {
            header[12] - 0xa0
        } else {
            header[12]
        };
        let s = header[13];
        let f = header[14];

        let msf = match Msf::from_bcd(m, s, f) {
            Some(msf) => msf,
            None => return Err(CdError::BadBcd),
        };

        let mode = match header[15] {
            0 => CdRomMode::Empty,
            1 => CdRomMode::Mode1,
            2 => CdRomMode::Mode2,
            _ => return Err(CdError::BadFormat),
        };

        Ok(CdRomHeader { msf, mode })
    }
*/
pub struct XaSubHeader(Vec<u8>);

impl XaSubHeader {
    /// Return the first File Number
    pub fn file_number(&self) -> u8 {
        self.0[0]
    }

    /// Return the first Channel Number
    pub fn channel_number(&self) -> u8 {
        self.0[1]
    }

    /// Return the first Submode
    pub fn submode(&self) -> XaSubmode {
        XaSubmode(self.0[2])
    }

    // Returns the first coding info based on the sector type
    // pub fn coding_info(&self) -> XaCodingInfo {
    //     let submode = self.submode();
    //     let coding = self.0[3];
    //
    //     if submode.video() {
    //         XaCodingInfo::Video(XaCodingVideo(coding))
    //     } else if submode.audio() {
    //         XaCodingInfo::Audio(XaCodingAudio(coding))
    //     } else {
    //         XaCodingInfo::Unknown(coding)
    //     }
    // }
}
pub struct XaSubmode(pub u8);

impl XaSubmode {
    /// True if the End Of Record (EOR) bit is set.
    pub fn end_of_record(self) -> bool {
        self.0 & 1 != 0
    }

    /// True if the Video (V) bit is set
    pub fn video(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// True if the Audio (A) bit is set.
    pub fn audio(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    /// True if the Data (D) bit is set.
    pub fn data(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    /// True if the Trigger (T) bit is set.
    pub fn trigger(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    /// Return the sector form
    pub fn form(self) -> XaForm {
        let form2 = self.0 & (1 << 5) != 0;

        if form2 {
            XaForm::Form2
        } else {
            XaForm::Form1
        }
    }

    /// True if the Real-Time Sector (RT) bit is set
    pub fn real_time(self) -> bool {
        self.0 & (1 << 6) != 0
    }

    /// True if the End Of File (EOF) bit is set
    pub fn end_of_file(self) -> bool {
        self.0 & (1 << 7) != 0
    }
}

pub enum XaForm {
    /// Mode 2 Form 1: 2048 bytes of data, 4 bytes of error detection and 276 bytes of error
    /// correction
    Form1 = 0,
    /// Mode 2 Form 2: 2324 bytes of data, 4 bytes of "quality control".
    ///
    /// The CDi spec says that those bytes are reserved and ignored by the system and *recommends*
    /// to use the same algorithm as for the Form 1 error detection code. It's also possible to
    /// leave it to zero if unused...
    Form2 = 1,
}

use std::{fs::File, io::{Read, Seek}, num::NonZero, sync::{Arc, Condvar, Mutex}, thread};

use crate::cdrom::{SECTOR_SIZE, Track};

// My take on https://github.com/simias/rustation-ng/blob/main/src/psx/cd/disc/cache.rs


pub struct CdCache {
    reader: Arc<(Mutex<Reader>, Condvar)>,
    handle: Option<thread::JoinHandle<()>>,
    tracks: Vec<Track>
}

impl CdCache {
    pub fn new(tracks: Vec<Track>) -> Self {
        let reader = Arc::new((Mutex::new(Reader::new(tracks.clone())), Condvar::new()));

        let thread_reader = reader.clone();

        let bldr = std::thread::Builder::new()
            .name("cdcache".to_string());

        let handle = bldr.spawn(move || {
            run_prefetcher( thread_reader);
        });

        CdCache {
            reader,
            handle: Some(handle.expect("ok")),
            tracks
        }
    }

    pub fn read_sector(&mut self, sector: u64) -> Option<[u8;0x930]> {
        let (reader_mutex, cond) = &*self.reader;
        let mut reader = reader_mutex.lock().unwrap();

        loop {
            reader.prefetch_next = sector;
            reader.prefetch_remaining = PREFETCH_READAHEAD_SECTORS;

            cond.notify_one();

            if let Some(s) = reader.sectors.get(&sector) {
                return s.clone();
            }
            reader = cond.wait(reader).unwrap();
        }


    }
}

impl Drop for CdCache {
    fn drop(&mut self) {
        let  (reader, cond) = &*self.reader;
        let mut reader = reader.lock().unwrap();

        reader.quit = true;
        cond.notify_one();
        if let Some(_) = self.handle.take() {
            // h.join().unwrap();
        }
    }
}

pub struct Reader {
    sectors: lru::LruCache<u64, Option<[u8;0x930]>>,
    prefetch_remaining: u32,
    prefetch_next: u64,
    quit: bool,
    tracks: Vec<Track>,
    files: Vec<Option<File>>,
}

impl Reader {
    pub fn new(tracks: Vec<Track>) -> Self {
        let mut files = Vec::with_capacity(tracks.capacity());
        for _ in &tracks {
            files.push(None);
        }
        Reader {
            quit: false,
            prefetch_remaining: 0,
            prefetch_next: 0,
            sectors: lru::LruCache::new(NonZero::new(CACHE_CAPACITY).unwrap()),
            tracks,
            files
        }
    }

    pub fn valid_sector(&self, sector: u64) -> bool {
        let track = self.tracks.iter().find(|x| {
            x.end_sector >= sector
        });
        track.is_some()
    }
    pub fn read_sector(&mut self, sector: u64, buff: &mut [u8]) {
        let track = self.tracks.iter().find(|x| {
            x.end_sector >= sector
        }).expect("ok");

        let maybefile = &mut self.files[track.id as usize - 1];
        let f = if maybefile.is_none() {
            maybefile.insert(File::open(&track.filename).expect("ok"))
        } else {
            maybefile.as_mut().unwrap()
        };

        if sector < track.start_sector {
            // we are in pregap
            buff.fill(0);
        } else {
            let rel_sector = sector - track.start_sector;
            f.seek(std::io::SeekFrom::Start(rel_sector * SECTOR_SIZE)).expect("ok");
            f.read(buff).expect("ok");// can cause problems on last sector?
        }

        
        // let track_idx = track.indexes.iter().find(|x| x.id != 0).unwrap();
        // let pos = sector - track_idx.lba;
    }
}

fn run_prefetcher( reader: Arc<(Mutex<Reader>, Condvar)>) {
    let (reader_mutex, cond) = &*reader;
    loop {
        let mut reader = reader_mutex.lock().unwrap();

        if reader.quit {
            break;
        }

        if reader.prefetch_remaining == 0 {
            reader = cond.wait(reader).unwrap();
            continue;
        }

        let sector = reader.prefetch_next;

        let next_sector = reader.prefetch_next + 0x930;
        if reader.valid_sector(next_sector) {
            reader.prefetch_remaining -= 1; // halp! will blow out at the end of the disk..
            reader.prefetch_next += 0x930;
        } else {
            reader.prefetch_remaining = 0;
        }

       
        if reader.sectors.contains(&sector) {
            continue;
        }

        // drop(reader);

        // read sector...
        // file.seek(std::io::SeekFrom::Start(pos as u64)).expect("ok");
        // file.read_exact(&mut buff).expect("ok");// can cause problems on last sector?


        // let mut reader = reader_mutex.lock().unwrap();
        let mut buff = [0u8;0x930];
        if reader.valid_sector(sector) {
            reader.read_sector(sector, &mut buff);
            reader.sectors.put(sector, Some(buff));
        } else {
            println!("WARN: attempt to read invalid sector");
            reader.sectors.put(sector, None);
        }

        cond.notify_one();
    }
}


const PREFETCH_READAHEAD_SECTORS: u32 = 5;

const CACHE_CAPACITY : usize = 1024;

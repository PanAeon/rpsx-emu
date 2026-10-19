use std::{fs::File, io::{Read, Seek}, num::NonZero, sync::{Arc, Condvar, Mutex}, thread};

use crate::cdrom::Track;

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

        let handle = std::thread::spawn(move || {
            run_prefetcher( thread_reader);
        });

        CdCache {
            reader,
            handle: Some(handle),
            tracks
        }
    }

    pub fn read_sector(&mut self, pos: usize) -> [u8;0x930] {
        let (reader_mutex, cond) = &*self.reader;
        let mut reader = reader_mutex.lock().unwrap();

        loop {
            reader.prefetch_next = pos;
            reader.prefetch_remaining = PREFETCH_READAHEAD_SECTORS;

            cond.notify_one();

            if let Some(s) = reader.sectors.get(&pos) {
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
    sectors: lru::LruCache<usize, [u8;0x930]>,
    prefetch_remaining: u32,
    prefetch_next: usize,
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
    pub fn read_sector(&mut self, pos: usize, buff: &mut [u8]) {
        // let track_idx = last_track.indexes.iter().find(|x| x.id != 0).unwrap();
        // first find track...
        let track = self.tracks.iter().find(|x| {
          let track_idx = x.indexes.iter().find(|x| x.id != 0).unwrap();
          let end = track_idx.lba + x.len;
          end >= pos
        }).expect("ok");
        let maybefile = &mut self.files[track.id as usize - 1];
        let f = if maybefile.is_none() {
            maybefile.insert(File::open(&track.filename).expect("ok"))
        } else {
            maybefile.as_mut().unwrap()
        };
        
        let track_idx = track.indexes.iter().find(|x| x.id != 0).unwrap();
        let pos = pos - track_idx.lba;
        f.seek(std::io::SeekFrom::Start(pos as u64)).expect("ok");
        f.read_exact(buff).expect("ok");// can cause problems on last sector?
            
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

        reader.prefetch_remaining -= 1; // halp! will blow out at the end of the disk..
        let pos = reader.prefetch_next;
        reader.prefetch_next += 0x930;
       
        if reader.sectors.contains(&pos) {
            continue;
        }

        // drop(reader);

        // read sector...
        // file.seek(std::io::SeekFrom::Start(pos as u64)).expect("ok");
        // file.read_exact(&mut buff).expect("ok");// can cause problems on last sector?


        // let mut reader = reader_mutex.lock().unwrap();
        let mut buff = [0u8;0x930];
        reader.read_sector(pos, &mut buff);
        reader.sectors.put(pos, buff);

        cond.notify_one();
    }
}


const PREFETCH_READAHEAD_SECTORS: u32 = 75;

const CACHE_CAPACITY : usize = 1024;

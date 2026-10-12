use std::{fs::File, io::{Read, Seek}, num::NonZero, sync::{Arc, Condvar, Mutex}, thread};

// My take on https://github.com/simias/rustation-ng/blob/main/src/psx/cd/disc/cache.rs


pub struct CdCache {
    reader: Arc<(Mutex<Reader>, Condvar)>,
    handle: thread::JoinHandle<()>,
    file: File
}

impl CdCache {
    pub fn new(file: File) -> Self {
        let reader = Arc::new((Mutex::new(Reader::new()), Condvar::new()));

        let thread_reader = reader.clone();
        let thread_file = file.try_clone().unwrap();

        let handle = std::thread::spawn(move || {
            run_prefetcher(thread_file, thread_reader);
        });

        CdCache {
            reader,
            handle,
            file
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
        // self.handle.join().unwrap();
    }
}

pub struct Reader {
    sectors: lru::LruCache<usize, [u8;0x930]>,
    prefetch_remaining: u32,
    prefetch_next: usize,
    quit: bool,
}

impl Reader {
    pub fn new() -> Self {
        Reader {
            quit: false,
            prefetch_remaining: 0,
            prefetch_next: 0,
            sectors: lru::LruCache::new(NonZero::new(CACHE_CAPACITY).unwrap()),
        }
    }
}

fn run_prefetcher(mut file: File, reader: Arc<(Mutex<Reader>, Condvar)>) {
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

        drop(reader);

        // read sector...
        file.seek(std::io::SeekFrom::Start(pos as u64)).expect("ok");
        let mut buff = [0u8;0x930];
        file.read_exact(&mut buff).expect("ok");// can cause problems on last sector?


        let mut reader = reader_mutex.lock().unwrap();
        reader.sectors.put(pos, buff);

        cond.notify_one();
    }
}

const PREFETCH_READAHEAD_SECTORS: u32 = 75;

const CACHE_CAPACITY : usize = 1024;

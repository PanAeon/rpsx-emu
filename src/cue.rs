use std::{fs::File, path::PathBuf, str::FromStr};

use log::info;
use serde::{Deserialize, Serialize};

use crate::cdrom::{SECTOR_SIZE, TrackType, mm_ss_ff};
// from https://github.com/kaezrr/starpsx/blob/main/cue/src/scanner.rs


#[derive(PartialEq, Eq, Serialize, Deserialize, Debug, Clone)]
pub struct TrackIndex {
    pub id: u8,
    pub start_pos: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Track {
    pub id: u8,
    pub track_type: TrackType,
    pub indexes: Vec<TrackIndex>,
    pub filename: String,
    // pub len: usize,
}
// #[derive(Debug)]
// pub struct File {
//     #[expect(unused)]
//     format: FileType,
//     path: PathBuf,
//     tracks: Vec<Track>,
// }

// #[derive(Debug)]
// pub enum FileType {
//     Binary,
// }
pub fn load_cue(cue: Vec<Track>) -> Vec<crate::cdrom::Track> {
    let mut tracks = Vec::new();
    let mut absolute_sector : u64 = 150;

    for track in cue {
        let metadata = File::open(&track.filename).unwrap().metadata().unwrap();
        let file_sector_count = metadata.len().saturating_div(SECTOR_SIZE as u64);

        let mut index_00 : Option<TrackIndex> = None;
        let mut index_01 : Option<TrackIndex> = None;

        for index in track.indexes {
            match index.id {
                0 => index_00 = Some(index),
                1 => index_01 = Some(index),
                _ => {}
            }
        }
        let Some(index_01) = index_01 else {
            panic!("index_01 not found");
        };

        let track_start = absolute_sector + index_01.start_pos;
        let pregap_start = if let Some(idx) = index_00 {
            absolute_sector + idx.start_pos
        } else {
            track_start
        };
        let track_end = track_start + file_sector_count;

        tracks.push(crate::cdrom::Track {
            id: track.id,
            pregap_sector: pregap_start,
            start_sector: track_start,
            end_sector: track_end,
            filename: track.filename,
            len: metadata.len(),
        });
        absolute_sector = track_end; // TODO: why different?
    }
    for track in &tracks {
        let (mm, ss, ff) = mm_ss_ff(track.start_sector);
            info!("track {}: {:02}:{:02}:{:02} lba={}", 
                track.id,
                mm,
                ss,
                ff,
                track.start_sector,
            );
    }
    tracks
}

// FILE "Crash Bandicoot (USA).bin" BINARY
//   TRACK 01 MODE2/2352
//     INDEX 01 00:00:00
pub fn parse_cue(path: &str) -> Vec<Track> {
    let data = std::fs::read(path).expect("ok");
    let scanner = Scanner::with_source(data);
    let tokens = scanner.tokenize().expect("ok");
    let mut dir_path = PathBuf::from_str(path).expect("ok");
    dir_path.pop();
    let parser = CueParser::new(dir_path, tokens);
    parser.parse_cuesheet().expect("ok")
}
pub struct CueParser {
    tokens: Vec<Token>,
    current: usize,
    path: PathBuf,
}

impl CueParser {
    pub const fn new(path: PathBuf, tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0, path }
    }

    /// cue -> file*
    pub fn parse_cuesheet(mut self) -> anyhow::Result<Vec<Track>> {
        let mut tracks = Vec::new();

        while !self.is_at_end() && self.peek() != &Token::Eof {
            tracks.append(&mut self.parse_file()?);
        }

        Ok(tracks)
    }

    fn parse_file(&mut self) -> anyhow::Result<Vec<Track>> {
        let Token::File = self.advance() else {
            anyhow::bail!("Expect 'FILE'.");
        };

        let Token::String(name) = self.advance().clone() else {
            anyhow::bail!("Expect file path.");
        };

        let mut  fullname = self.path.clone();
        fullname.push(name);

        let _ = match self.advance() {
            Token::Binary => (),//FileType::Binary,
            t => anyhow::bail!("File type {t:?} not implemented."),
        };

        let Token::Newline = self.advance() else {
            anyhow::bail!("Expect newline after file.");
        };

        let mut tracks = Vec::new();

        while matches!(self.peek(), Token::Track) {
            tracks.push(self.parse_track(fullname.to_str().expect("ok").to_string())?);
        }

        Ok(tracks)
    }

    /// track -> "TRACK" tracknumber tracktype "\n" flags? index*
    fn parse_track(&mut self, filename: String) -> anyhow::Result<Track> {
        let Token::Track = self.advance() else {
            anyhow::bail!("Expect 'TRACK'.");
        };

        let &Token::Number(id) = self.advance() else {
            anyhow::bail!("Expect track id.");
        };

        let track_type = match self.advance() {
            Token::Audio => TrackType::Audio,
            Token::Mode2_2352 => TrackType::Mode2352,
            t => anyhow::bail!("Track type {t:?} not implemented."),
        };

        let Token::Newline = self.advance() else {
            anyhow::bail!("Expect newline after track.");
        };

        let mut indexes = Vec::new();

        // Consume useless flags
        if matches!(self.peek(), Token::Flags) {
            self.parse_flags()?;
        }

        while matches!(self.peek(), Token::Index) {
            indexes.push(self.parse_index()?);
        }

        // let len = File::open(&filename).unwrap().metadata().unwrap().len();


        Ok(Track {
            id,
            indexes,
            track_type,
            filename,
            // len: len as usize
        })
    }

    /// flags -> "FLAGS" "DCP" "\n"
    fn parse_flags(&mut self) -> anyhow::Result<()> {
        let Token::Flags = self.advance() else {
            anyhow::bail!("Expect 'FLAGS'.");
        };

        let Token::Dcp = self.advance() else {
            anyhow::bail!("Expect 'DCP'");
        };

        let Token::Newline = self.advance() else {
            anyhow::bail!("Expect newline after flags.");
        };

        Ok(())
    }

    /// index -> "INDEX" indexnumber sector "\n"
    fn parse_index(&mut self) -> anyhow::Result<TrackIndex> {
        let Token::Index = self.advance() else {
            anyhow::bail!("Expect 'INDEX'.");
        };

        let &Token::Number(id) = self.advance() else {
            anyhow::bail!("Expect index number.");
        };

        let &Token::CdTime(lba) = self.advance() else {
            anyhow::bail!("Expect track time.");
        };

        let Token::Newline = self.advance() else {
            anyhow::bail!("Expect newline after index.");
        };

        Ok(TrackIndex { id, start_pos: lba })
    }

    const fn is_at_end(&self) -> bool {
        self.current >= self.tokens.len()
    }

    fn advance(&mut self) -> &Token {
        self.current += 1;
        &self.tokens[self.current - 1]
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }
}


pub struct Scanner {
    source: Vec<u8>,
    tokens: Vec<Token>,
    start: usize,
    current: usize,
}

impl Scanner {
    pub const fn with_source(source: Vec<u8>) -> Self {
        Self {
            source,
            tokens: Vec::new(),
            start: 0,
            current: 0,
        }
    }

    pub fn tokenize(mut self) -> anyhow::Result<Vec<Token>> {
        while !self.is_at_end() {
            self.scan_token()?;
            self.start = self.current;
        }

        self.tokens.push(Token::Eof);

        Ok(self.tokens)
    }

    fn scan_token(&mut self) -> anyhow::Result<()> {
        let token = match self.advance() {
            ' ' | '\t' | '\r' => return Ok(()),

            '\n' => Token::Newline,

            '"' => Token::String(self.string()?),

            c if c.is_ascii_alphabetic() => self.keyword()?,

            c if c.is_ascii_digit() => self.number_or_time()?,

            c => anyhow::bail!("Unexpected character '{c}'"),
        };

        self.tokens.push(token);

        Ok(())
    }

    fn string(&mut self) -> anyhow::Result<String> {
        while !self.is_at_end() && self.peek() != '"' {
            self.advance();
        }

        if self.is_at_end() {
            anyhow::bail!("Unterminated string");
        }

        self.advance();

        let bytes = &self.source[self.start + 1..self.current - 1];
        let word = str::from_utf8(bytes)?;

        Ok(word.to_string())
    }

    fn keyword(&mut self) -> anyhow::Result<Token> {
        while is_alnum(self.peek()) {
            self.advance();
        }

        let bytes = &self.source[self.start..self.current];
        let keyword = str::from_utf8(bytes).expect("UTF-8");

        try_to_keyword(keyword)
    }

    fn number_or_time(&mut self) -> anyhow::Result<Token> {
        while self.peek().is_ascii_digit() {
            self.advance();
        }

        // If not followed by ':', it's a plain number
        if self.peek() != ':' {
            let bytes = &self.source[self.start..self.current];
            let word = std::str::from_utf8(bytes)?;
            return Ok(Token::Number(word.parse()?));
        }

        // We are parsing MM:SS:FF
        for _ in 0..2 {
            self.advance(); // consume ':'

            if !self.peek().is_ascii_digit() {
                anyhow::bail!("Invalid CD time format");
            }

            while self.peek().is_ascii_digit() {
                self.advance();
            }
        }

        let bytes = &self.source[self.start..self.current];
        let word = std::str::from_utf8(bytes)?;

        Ok(Token::CdTime(to_sectors(
            word[0..2].parse()?, // MM
            word[3..5].parse()?, // HH
            word[6..8].parse()?, // SS
        )))
    }

    fn peek(&self) -> char {
        self.source[self.current] as char
    }

    fn advance(&mut self) -> char {
        let ch = self.source[self.current];
        self.current += 1;
        ch as char
    }

    const fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }
}

const fn is_alnum(ch: char) -> bool {
    ch == '/' || ch.is_ascii_alphanumeric()
}

fn try_to_keyword(s: &str) -> anyhow::Result<Token> {
    match s {
        "FILE" => Ok(Token::File),
        "BINARY" => Ok(Token::Binary),
        "TRACK" => Ok(Token::Track),
        "INDEX" => Ok(Token::Index),
        "AUDIO" => Ok(Token::Audio),
        "FLAGS" => Ok(Token::Flags),
        "DCP" => Ok(Token::Dcp),
        "MODE2/2352" => Ok(Token::Mode2_2352),
        word => anyhow::bail!("Invalid keyword: -{word}-"),
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Token {
    File,
    Track,
    Index,

    // File types
    Binary,

    // Track types
    Audio,
    Mode2_2352,

    String(String),
    Number(u8),
    CdTime(u64),

    Newline,
    Eof,

    // Random useless stuff
    Flags,
    Dcp,
}

const fn to_sectors(minutes: usize, seconds: usize, frames: usize) -> u64 {
    ((minutes * 60 * 75 + seconds * 75 + frames) ) as u64
}



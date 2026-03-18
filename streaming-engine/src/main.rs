use serde::Serialize;
use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::c_char;
use webm_iterable::matroska_spec::MatroskaSpec;
use webm_iterable::WebmIterator;

extern "C" {
    fn init_audio(
        codec_id: i32,
        sample_rate: i32,
        channels: i32,
        extradata: *const u8,
        extradata_size: i32,
    ) -> i32;
    fn decode_audio(data_ptr: *const u8, data_size: i32) -> i32;
}

#[derive(Serialize)]
pub struct MkvTrackInfo {
    pub track_number: u64,
    pub track_type: String,
    pub codec_id: String,
    pub default_duration: u64,
    pub codec_string: String,
    pub width: u64,
    pub height: u64,
    pub language: String,
    pub sample_rate: f64,
    pub channels: u64,
    pub codec_private: Vec<u8>,
}

#[derive(Serialize)]
pub struct MkvHeader {
    pub duration: f64,
    pub tracks: Vec<MkvTrackInfo>,
    pub cues_position: Option<u64>,
}

#[derive(Serialize)]
pub struct CuePoint {
    pub time: f64,
    pub offset: u64,
}

// --- NEW ZERO-COPY MEMORY ALLOCATORS ---

#[no_mangle]
pub extern "C" fn alloc_memory(size: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

// Free memory created by alloc_memory
#[no_mangle]
pub extern "C" fn free_memory(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        // We tell Rust the length was 0, but the capacity was `size`.
        let _buf = Vec::from_raw_parts(ptr, 0, size);
    }
}

// Free the MP4 segments returned by the Demuxer
#[no_mangle]
pub extern "C" fn free_segment(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        // Safely reconstruct the boxed slice and let it drop instantly
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(ptr, size));
    }
}

#[no_mangle]
pub extern "C" fn get_mkv_info_fast_json(ptr: *const u8, len: usize) -> *mut c_char {
    let mkv_data = unsafe { std::slice::from_raw_parts(ptr, len) };
    let iterator = WebmIterator::new(mkv_data, &[]);
    let mut tracks: Vec<MkvTrackInfo> = Vec::new();

    let mut timecode_scale = 1_000_000u64;
    let mut duration_raw = 0.0;
    let mut cues_pos: Option<u64> = None;

    let mut in_seek = false;
    let mut current_seek_id = Vec::new();

    let mut t_num = 0;
    let mut t_type = String::new();
    let mut t_codec = String::new();
    let mut t_dur = 0;
    let mut t_width = 0;
    let mut t_height = 0;
    let mut t_private = Vec::new();
    let mut t_lang = String::from("und");

    let mut t_sample_rate = 48000.0;
    let mut t_channels = 2;

    let get_codec_string = |codec: &str, private_data: &[u8]| -> String {
        if codec == "V_MPEG4/ISO/AVC" {
            if private_data.len() >= 4 {
                format!(
                    "avc1.{:02x}{:02x}{:02x}",
                    private_data[1], private_data[2], private_data[3]
                )
            } else {
                "avc1.4d401e".to_string()
            }
        } else if codec == "V_MPEGH/ISO/HEVC" {
            "hev1.1.6.L93.B0".to_string()
        } else if codec == "A_AAC" {
            if private_data.len() >= 2 {
                let object_type = (private_data[0] & 0xF8) >> 3;
                format!("mp4a.40.{}", object_type)
            } else {
                "mp4a.40.2".to_string()
            }
        }
        // --- AUDIO CODECS ---
        else if codec == "A_AC3" {
            "ac-3".to_string()
        } else if codec == "A_EAC3" {
            "ec-3".to_string()
        } else if codec == "A_TRUEHD" {
            "truehd".to_string()
        } else if codec == "A_DTS" {
            "dts".to_string()
        } else if codec == "A_FLAC" {
            "flac".to_string()
        } else if codec == "A_OPUS" {
            "opus".to_string()
        } else {
            "unknown".to_string()
        }
    };

    for element in iterator {
        if let Ok(MatroskaSpec::Cluster(_)) = element {
            break;
        }
        match element {
            Ok(MatroskaSpec::TimestampScale(scale)) => timecode_scale = scale,
            Ok(MatroskaSpec::Duration(dur)) => duration_raw = dur,
            Ok(MatroskaSpec::Seek(_)) => {
                in_seek = true;
                current_seek_id.clear();
            }
            Ok(MatroskaSpec::SeekID(id)) => {
                if in_seek {
                    current_seek_id = id;
                }
            }
            Ok(MatroskaSpec::SeekPosition(pos)) => {
                if in_seek && current_seek_id == vec![0x1C, 0x53, 0xBB, 0x6B] {
                    cues_pos = Some(pos);
                }
            }
            Ok(MatroskaSpec::TrackEntry(_)) => {
                if t_num > 0 {
                    let final_codec = get_codec_string(&t_codec, &t_private);
                    tracks.push(MkvTrackInfo {
                        track_number: t_num,
                        track_type: t_type.clone(),
                        codec_id: t_codec.clone(),
                        default_duration: t_dur,
                        codec_string: final_codec,
                        width: t_width,
                        height: t_height,
                        language: t_lang,
                        sample_rate: t_sample_rate,
                        channels: t_channels,
                        codec_private: t_private.clone(),
                    });
                }
                t_num = 0;
                t_type = "unknown".into();
                t_codec = "unknown".into();
                t_dur = 0;
                t_width = 0;
                t_height = 0;
                t_private.clear();
                t_lang = "und".into();
                t_sample_rate = 48000.0;
                t_channels = 2;
            }
            Ok(MatroskaSpec::TrackNumber(num)) => t_num = num,
            Ok(MatroskaSpec::TrackType(t)) => {
                t_type = if t == 1 {
                    "video".into()
                } else if t == 2 {
                    "audio".into()
                } else {
                    "other".into()
                }
            }
            Ok(MatroskaSpec::CodecID(id)) => t_codec = id,
            Ok(MatroskaSpec::CodecPrivate(data)) => t_private = data,
            Ok(MatroskaSpec::DefaultDuration(d)) => t_dur = d,
            Ok(MatroskaSpec::PixelWidth(w)) => t_width = w,
            Ok(MatroskaSpec::PixelHeight(h)) => t_height = h,
            Ok(MatroskaSpec::Language(lang)) => t_lang = lang,
            Ok(MatroskaSpec::SamplingFrequency(freq)) => t_sample_rate = freq,
            Ok(MatroskaSpec::Channels(ch)) => t_channels = ch,
            _ => {}
        }
    }

    if t_num > 0 {
        let final_codec = get_codec_string(&t_codec, &t_private);
        tracks.push(MkvTrackInfo {
            track_number: t_num,
            track_type: t_type,
            codec_id: t_codec,
            default_duration: t_dur,
            codec_string: final_codec,
            width: t_width,
            height: t_height,
            language: t_lang,
            sample_rate: t_sample_rate,
            channels: t_channels,
            codec_private: t_private,
        });
    }

    let final_duration = (duration_raw * timecode_scale as f64) / 1_000_000_000.0;
    let header = MkvHeader {
        duration: final_duration,
        tracks,
        cues_position: cues_pos,
    };

    let json_string = serde_json::to_string(&header).unwrap_or_else(|_| "{}".to_string());
    let c_str = CString::new(json_string).unwrap();
    c_str.into_raw()
}

#[no_mangle]
pub extern "C" fn free_string(s: *mut c_char) {
    unsafe {
        if s.is_null() {
            return;
        }
        let _ = CString::from_raw(s);
    }
}

#[no_mangle]
pub extern "C" fn parse_cues_json(ptr: *const u8, len: usize) -> *mut std::os::raw::c_char {
    let cues_data = unsafe { std::slice::from_raw_parts(ptr, len) };

    let mut start_offset = 0;
    if cues_data.len() > 4 {
        for i in 0..cues_data.len() - 4 {
            if cues_data[i] == 0x1C
                && cues_data[i + 1] == 0x53
                && cues_data[i + 2] == 0xBB
                && cues_data[i + 3] == 0x6B
            {
                start_offset = i;
                break;
            }
        }
    }

    let iterator = WebmIterator::new(&cues_data[start_offset..], &[]);
    let mut cue_points: Vec<CuePoint> = Vec::new();
    let mut current_time = 0.0;

    for element in iterator {
        match element {
            Ok(MatroskaSpec::CueTime(t)) => current_time = t as f64,
            Ok(MatroskaSpec::CueClusterPosition(pos)) => {
                cue_points.push(CuePoint {
                    time: current_time / 1000.0,
                    offset: pos,
                });
            }
            Err(_) => break,
            _ => {}
        }
    }

    let json_string = serde_json::to_string(&cue_points).unwrap_or_else(|_| "[]".to_string());
    let c_str = std::ffi::CString::new(json_string).unwrap();
    c_str.into_raw()
}

struct Frame {
    data: Vec<u8>,
    is_keyframe: bool,
    size: u32,
    dts: u64,
    ctts: i32,
}

pub struct Demuxer {
    buffer: Vec<u8>,
    video_track_id: u64,
    audio_track_id: u64,
    codec_id: String,
    pub audio_codec_id: String,
    pub needs_transcode: bool,
    width: u16,
    height: u16,
    codec_private: Vec<u8>,
    audio_private: Vec<u8>,
    audio_sample_rate: u32,
    audio_channels: u16,
    initialized: bool,
    seq_number: u32,
    base_decode_time: u64,
    audio_base_decode_time: u64,
    frame_duration: u64,
    staged_video_frames: Vec<Frame>,
    staged_audio_frames: Vec<Frame>,
    raw_audio_queue: Vec<Frame>, // Waiting room for AC3
    last_audio_dts: u64,
    last_segment_len: usize,
    current_cluster_time: u64,
    needs_sync: bool,
}

impl Demuxer {
    pub fn new(
        video_track_id: u64,
        audio_track_id: u64,
        width: u16,
        height: u16,
        _duration_ms: f64,
        codec_id: String,
    ) -> Demuxer {
        Demuxer {
            buffer: Vec::new(),
            video_track_id,
            audio_track_id,
            codec_id,
            audio_codec_id: String::new(),
            needs_transcode: true,
            width,
            height,
            codec_private: Vec::new(),
            audio_private: Vec::new(),
            audio_sample_rate: 48000,
            audio_channels: 2,
            initialized: false,
            seq_number: 1,
            base_decode_time: 0,
            audio_base_decode_time: 0,
            frame_duration: 3753,
            staged_video_frames: Vec::new(),
            staged_audio_frames: Vec::new(),
            raw_audio_queue: Vec::new(),
            last_audio_dts: 0,
            last_segment_len: 0,
            current_cluster_time: 0,
            needs_sync: true,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.staged_video_frames.clear();
        self.staged_audio_frames.clear();
        self.raw_audio_queue.clear();

        self.base_decode_time = 0;
        self.audio_base_decode_time = 0;
        self.needs_sync = true;
    }

    pub fn init(&mut self, chunk: &[u8]) -> Result<Vec<u8>, String> {
        self.buffer.extend_from_slice(chunk);
        let iterator = WebmIterator::new(&self.buffer[..], &[]);
        let mut t_num = 0;
        let mut t_dur_ns = 0;

        let mut audio_codec = String::new();

        for element in iterator {
            if let Ok(MatroskaSpec::Cluster(_)) = element {
                break;
            }
            match element {
                Ok(MatroskaSpec::TrackEntry(_)) => {
                    if t_num == self.video_track_id && t_dur_ns > 0 {
                        self.frame_duration = (t_dur_ns * 90000) / 1_000_000_000;
                    }
                    t_num = 0;
                    t_dur_ns = 0;
                }
                Ok(MatroskaSpec::TrackNumber(num)) => t_num = num,

                Ok(MatroskaSpec::CodecID(id)) => {
                    if t_num == self.audio_track_id {
                        audio_codec = id.clone();
                        self.audio_codec_id = id;
                    }
                }

                Ok(MatroskaSpec::CodecPrivate(data)) => {
                    if t_num == self.video_track_id {
                        self.codec_private = data.clone();
                    } else if t_num == self.audio_track_id {
                        self.audio_private = data.clone();
                    }
                }
                Ok(MatroskaSpec::DefaultDuration(d)) => t_dur_ns = d,
                Ok(MatroskaSpec::SamplingFrequency(freq)) => {
                    if t_num == self.audio_track_id {
                        self.audio_sample_rate = freq as u32;
                    }
                }
                Ok(MatroskaSpec::Channels(ch)) => {
                    if t_num == self.audio_track_id {
                        self.audio_channels = ch as u16;
                    }
                }
                _ => {}
            }
        }

        // Start the C decoder
        if self.audio_track_id > 0 {
            let private_ptr = if self.audio_private.is_empty() {
                std::ptr::null()
            } else {
                self.audio_private.as_ptr()
            };
            let private_len = self.audio_private.len() as i32;

            // PASS THE REAL FFMPEG CODEC ID
            let c_codec_id = if audio_codec == "A_AC3" {
                86019
            } else if audio_codec == "A_EAC3" {
                86057
            } else if audio_codec == "A_TRUEHD" {
                86060
            } else {
                0
            };

            unsafe {
                init_audio(
                    c_codec_id,
                    self.audio_sample_rate as i32,
                    self.audio_channels as i32,
                    private_ptr,
                    private_len,
                );
            }

            self.audio_channels = 2;
            self.audio_sample_rate = 48000;
        }

        if self.codec_private.is_empty() {
            return Err("No Video CodecPrivate found".into());
        }
        if self.codec_id == "V_MPEG4/ISO/AVC"
            && self.codec_private.len() > 3
            && self.codec_private[1] == 0x53
        {
            self.codec_private[1] = 0x64;
        }

        self.initialized = true;
        self.buffer.clear();

        let mut mp4 = Vec::new();
        let ftyp = start_box(&mut mp4, b"ftyp");
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&512u32.to_be_bytes());
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(b"iso2");
        mp4.extend_from_slice(b"mp41");
        end_box(&mut mp4, ftyp);

        let moov = start_box(&mut mp4, b"moov");
        let mvhd = start_box(&mut mp4, b"mvhd");
        mp4.extend_from_slice(&[0; 12]);
        mp4.extend_from_slice(&1000u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&0x00020000u32.to_be_bytes());
        mp4.extend_from_slice(&0x0100u16.to_be_bytes());
        mp4.extend_from_slice(&[0; 70]);

        let next_track_id = if self.audio_track_id > 0 { 3u32 } else { 2u32 };
        mp4.extend_from_slice(&next_track_id.to_be_bytes());
        end_box(&mut mp4, mvhd);

        let mvex = start_box(&mut mp4, b"mvex");
        let trex_vid = start_box(&mut mp4, b"trex");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&1u32.to_be_bytes());
        mp4.extend_from_slice(&1u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, trex_vid);

        if self.audio_track_id > 0 {
            let trex_aud = start_box(&mut mp4, b"trex");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&2u32.to_be_bytes());
            mp4.extend_from_slice(&1u32.to_be_bytes());
            mp4.extend_from_slice(&0u32.to_be_bytes());
            mp4.extend_from_slice(&0u32.to_be_bytes());
            mp4.extend_from_slice(&0u32.to_be_bytes());
            end_box(&mut mp4, trex_aud);
        }
        end_box(&mut mp4, mvex);

        let trak = start_box(&mut mp4, b"trak");
        let tkhd = start_box(&mut mp4, b"tkhd");
        mp4.extend_from_slice(&[0, 0, 0, 3]);
        mp4.extend_from_slice(&[0; 8]);
        mp4.extend_from_slice(&1u32.to_be_bytes());
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&[0; 16]);
        mp4.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00,
        ]);
        mp4.extend_from_slice(&((self.width as u32) << 16).to_be_bytes());
        mp4.extend_from_slice(&((self.height as u32) << 16).to_be_bytes());
        end_box(&mut mp4, tkhd);
        let mdia = start_box(&mut mp4, b"mdia");
        let mdhd = start_box(&mut mp4, b"mdhd");
        mp4.extend_from_slice(&[0; 12]);
        mp4.extend_from_slice(&90000u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&[0x15, 0xc7, 0, 0]);
        end_box(&mut mp4, mdhd);
        let hdlr = start_box(&mut mp4, b"hdlr");
        mp4.extend_from_slice(&[0; 8]);
        mp4.extend_from_slice(b"vide");
        mp4.extend_from_slice(&[0; 12]);
        mp4.extend_from_slice(b"VideoHandler\0");
        end_box(&mut mp4, hdlr);
        let minf = start_box(&mut mp4, b"minf");
        let vmhd = start_box(&mut mp4, b"vmhd");
        mp4.extend_from_slice(&[0, 0, 0, 1]);
        mp4.extend_from_slice(&[0; 8]);
        end_box(&mut mp4, vmhd);
        let dinf = start_box(&mut mp4, b"dinf");
        let dref = start_box(&mut mp4, b"dref");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&1u32.to_be_bytes());
        let url = start_box(&mut mp4, b"url ");
        mp4.extend_from_slice(&[0, 0, 0, 1]);
        end_box(&mut mp4, url);
        end_box(&mut mp4, dref);
        end_box(&mut mp4, dinf);
        let stbl = start_box(&mut mp4, b"stbl");
        let stsd = start_box(&mut mp4, b"stsd");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&1u32.to_be_bytes());

        let is_hevc = self.codec_id == "V_MPEGH/ISO/HEVC";
        let sample_entry_tag = if is_hevc { b"hev1" } else { b"avc1" };
        let config_box_tag = if is_hevc { b"hvcC" } else { b"avcC" };

        let sample_entry = start_box(&mut mp4, sample_entry_tag);
        mp4.extend_from_slice(&[0; 6]);
        mp4.extend_from_slice(&1u16.to_be_bytes());
        mp4.extend_from_slice(&[0; 16]);
        mp4.extend_from_slice(&self.width.to_be_bytes());
        mp4.extend_from_slice(&self.height.to_be_bytes());
        mp4.extend_from_slice(&[0x00, 0x48, 0x00, 0x00]);
        mp4.extend_from_slice(&[0x00, 0x48, 0x00, 0x00]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&1u16.to_be_bytes());
        mp4.extend_from_slice(&[0; 32]);
        mp4.extend_from_slice(&0x0018u16.to_be_bytes());
        mp4.extend_from_slice(&(-1i16).to_be_bytes());
        write_box_header(
            &mut mp4,
            config_box_tag,
            (8 + self.codec_private.len()) as u32,
        );
        mp4.extend_from_slice(&self.codec_private);
        end_box(&mut mp4, sample_entry);
        end_box(&mut mp4, stsd);

        let stts = start_box(&mut mp4, b"stts");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, stts);
        let ctts = start_box(&mut mp4, b"ctts");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, ctts);
        let stsz = start_box(&mut mp4, b"stsz");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, stsz);
        let stco = start_box(&mut mp4, b"stco");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, stco);
        let stsc = start_box(&mut mp4, b"stsc");
        mp4.extend_from_slice(&[0; 4]);
        mp4.extend_from_slice(&0u32.to_be_bytes());
        end_box(&mut mp4, stsc);
        end_box(&mut mp4, stbl);
        end_box(&mut mp4, minf);
        end_box(&mut mp4, mdia);
        end_box(&mut mp4, trak);

        if self.audio_track_id > 0 {
            let a_trak = start_box(&mut mp4, b"trak");
            let a_tkhd = start_box(&mut mp4, b"tkhd");
            mp4.extend_from_slice(&[0, 0, 0, 7]);
            mp4.extend_from_slice(&[0; 8]);
            mp4.extend_from_slice(&2u32.to_be_bytes());
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u32.to_be_bytes());
            mp4.extend_from_slice(&[0; 16]);
            mp4.extend_from_slice(&[
                0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00,
                0, 0, 0, 0, 0, 0, 0, 0,
            ]);
            end_box(&mut mp4, a_tkhd);

            let a_mdia = start_box(&mut mp4, b"mdia");
            let a_mdhd = start_box(&mut mp4, b"mdhd");
            mp4.extend_from_slice(&[0; 12]);
            mp4.extend_from_slice(&self.audio_sample_rate.to_be_bytes());
            mp4.extend_from_slice(&0u32.to_be_bytes());
            mp4.extend_from_slice(&[0x15, 0xc7, 0, 0]);
            end_box(&mut mp4, a_mdhd);

            let a_hdlr = start_box(&mut mp4, b"hdlr");
            mp4.extend_from_slice(&[0; 8]);
            mp4.extend_from_slice(b"soun");
            mp4.extend_from_slice(&[0; 12]);
            mp4.extend_from_slice(b"SoundHandler\0");
            end_box(&mut mp4, a_hdlr);

            let a_minf = start_box(&mut mp4, b"minf");
            let smhd = start_box(&mut mp4, b"smhd");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u16.to_be_bytes());
            mp4.extend_from_slice(&0u16.to_be_bytes());
            end_box(&mut mp4, smhd);
            let a_dinf = start_box(&mut mp4, b"dinf");
            let a_dref = start_box(&mut mp4, b"dref");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&1u32.to_be_bytes());
            let a_url = start_box(&mut mp4, b"url ");
            mp4.extend_from_slice(&[0, 0, 0, 1]);
            end_box(&mut mp4, a_url);
            end_box(&mut mp4, a_dref);
            end_box(&mut mp4, a_dinf);

            let a_stbl = start_box(&mut mp4, b"stbl");
            let a_stsd = start_box(&mut mp4, b"stsd");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&1u32.to_be_bytes());

            let mp4a = start_box(&mut mp4, b"mp4a");
            mp4.extend_from_slice(&[0; 6]);
            mp4.extend_from_slice(&1u16.to_be_bytes());
            mp4.extend_from_slice(&[0; 8]);
            mp4.extend_from_slice(&self.audio_channels.to_be_bytes());
            mp4.extend_from_slice(&16u16.to_be_bytes());
            mp4.extend_from_slice(&0u16.to_be_bytes());
            mp4.extend_from_slice(&0u16.to_be_bytes());
            mp4.extend_from_slice(&(self.audio_sample_rate << 16).to_be_bytes());

            let esds = start_box(&mut mp4, b"esds");
            mp4.extend_from_slice(&[0; 4]);

            //  AAC CONFIGURATOR
            let mut dynamic_aac_config = vec![0x11, 0x90]; // Default 48k Stereo

            if self.needs_transcode || self.audio_private.is_empty() {
                let sr_idx = match self.audio_sample_rate {
                    96000 => 0,
                    88200 => 1,
                    64000 => 2,
                    48000 => 3,
                    44100 => 4,
                    32000 => 5,
                    24000 => 6,
                    22050 => 7,
                    16000 => 8,
                    12000 => 9,
                    11025 => 10,
                    8000 => 11,
                    7350 => 12,
                    _ => 3, // fallback to 48k
                };

                // AAC-LC Object Type is 2. (2 << 3 = 16)
                let asc1 = 16 | (sr_idx >> 1);
                let asc2 = ((sr_idx & 1) << 7) | ((self.audio_channels as u8) << 3);
                dynamic_aac_config = vec![asc1, asc2];
            }

            let (private_bytes, private_len) =
                if self.needs_transcode || self.audio_private.is_empty() {
                    (dynamic_aac_config.as_slice(), 2u8)
                } else {
                    (
                        self.audio_private.as_slice(),
                        self.audio_private.len() as u8,
                    )
                };

            let dec_config_len = 13 + 2 + private_len;
            let es_desc_len = 3 + 2 + dec_config_len + 3;

            mp4.extend_from_slice(&[0x03, es_desc_len]);
            mp4.extend_from_slice(&[0x00, 0x00, 0x00]);
            mp4.extend_from_slice(&[0x04, dec_config_len]);
            mp4.extend_from_slice(&[0x40, 0x15]);
            mp4.extend_from_slice(&[0x00, 0x00, 0x00]);
            mp4.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            mp4.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

            mp4.extend_from_slice(&[0x05, private_len]);
            mp4.extend_from_slice(private_bytes);

            mp4.extend_from_slice(&[0x06, 0x01, 0x02]);
            end_box(&mut mp4, esds);

            end_box(&mut mp4, mp4a);
            end_box(&mut mp4, a_stsd);

            let a_stts = start_box(&mut mp4, b"stts");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u32.to_be_bytes());
            end_box(&mut mp4, a_stts);
            let a_stsz = start_box(&mut mp4, b"stsz");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u32.to_be_bytes());
            mp4.extend_from_slice(&0u32.to_be_bytes());
            end_box(&mut mp4, a_stsz);
            let a_stco = start_box(&mut mp4, b"stco");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u32.to_be_bytes());
            end_box(&mut mp4, a_stco);
            let a_stsc = start_box(&mut mp4, b"stsc");
            mp4.extend_from_slice(&[0; 4]);
            mp4.extend_from_slice(&0u32.to_be_bytes());
            end_box(&mut mp4, a_stsc);

            end_box(&mut mp4, a_stbl);
            end_box(&mut mp4, a_minf);
            end_box(&mut mp4, a_mdia);
            end_box(&mut mp4, a_trak);
        }

        end_box(&mut mp4, moov);
        Ok(mp4)
    }

    pub fn parse_chunk(&mut self, chunk: &[u8], is_final: bool) -> usize {
        if !self.initialized {
            return 0;
        }

        self.buffer.extend_from_slice(chunk);

        if self.needs_sync {
            let mut sync_idx = None;
            for i in 0..self.buffer.len().saturating_sub(3) {
                if self.buffer[i] == 0x1F
                    && self.buffer[i + 1] == 0x43
                    && self.buffer[i + 2] == 0xB6
                    && self.buffer[i + 3] == 0x75
                {
                    sync_idx = Some(i);
                    break;
                }
            }
            match sync_idx {
                Some(idx) => {
                    self.buffer.drain(0..idx);
                    self.needs_sync = false;
                }
                None => {
                    if self.buffer.len() > 3 {
                        let keep = self.buffer.len() - 3;
                        self.buffer.drain(0..keep);
                    }
                    return 0;
                }
            }
        }

        let mut last_valid_idx = 0;
        let mut found_cluster = false;

        if self.buffer.len() > 10 {
            for i in (0..self.buffer.len() - 4).rev() {
                if self.buffer[i] == 0x1F
                    && self.buffer[i + 1] == 0x43
                    && self.buffer[i + 2] == 0xB6
                    && self.buffer[i + 3] == 0x75
                {
                    last_valid_idx = i;
                    found_cluster = true;
                    break;
                }
            }
        }

        let process_limit = if found_cluster { last_valid_idx } else { 0 };
        let force_process = is_final || self.buffer.len() > 60 * 1024 * 1024;
        let actual_limit = if force_process {
            self.buffer.len()
        } else {
            process_limit
        };

        if actual_limit > 0 {
            let iterator = WebmIterator::new(&self.buffer[0..actual_limit], &[]);
            let mut safety = 0;

            for element in iterator {
                safety += 1;
                if safety > 5_000_000 {
                    break;
                }

                match element {
                    Ok(MatroskaSpec::Timestamp(tc)) => self.current_cluster_time = tc,
                    Ok(MatroskaSpec::SimpleBlock(block_data)) => {
                        let (track_id, header_len) = read_vint(&block_data);
                        if block_data.len() > header_len + 3 {
                            let offset = header_len + 3;
                            let size = (block_data.len() - offset) as u32;
                            if size > 0 {
                                let t_bytes = [block_data[header_len], block_data[header_len + 1]];
                                let rel_time = i16::from_be_bytes(t_bytes);

                                let mut abs_pts_ms_i64 =
                                    self.current_cluster_time as i64 + rel_time as i64;
                                if abs_pts_ms_i64 < 0 {
                                    abs_pts_ms_i64 = 0;
                                }
                                let abs_pts_ms = abs_pts_ms_i64 as u64;

                                if track_id == self.video_track_id {
                                    let is_keyframe = (block_data[header_len + 2] & 0x80) == 0x80;
                                    let pts = abs_pts_ms * 90;

                                    if self.base_decode_time > 0 {
                                        let gap = pts as i64 - self.base_decode_time as i64;
                                        if gap > 45000 {
                                            self.base_decode_time = pts - self.frame_duration;
                                        }
                                    } else {
                                        self.base_decode_time = pts;
                                    }

                                    let dts = self.base_decode_time;
                                    self.base_decode_time += self.frame_duration;
                                    let ctts = (pts as i64 - dts as i64) as i32;

                                    self.staged_video_frames.push(Frame {
                                        size,
                                        data: block_data[offset..].to_vec(),
                                        is_keyframe,
                                        dts,
                                        ctts,
                                    });
                                } else if track_id == self.audio_track_id {
                                    let frames = parse_lacing(&block_data, header_len);

                                    for frame_data in frames {
                                        // THE PACEMAKER
                                        let expected_ms = if self.audio_sample_rate > 0 {
                                            (self.audio_base_decode_time * 1000)
                                                / self.audio_sample_rate as u64
                                        } else {
                                            0
                                        };

                                        let gap = (abs_pts_ms as i64) - (expected_ms as i64);

                                        if self.audio_base_decode_time == 0
                                            || gap > 500
                                            || gap < -500
                                        {
                                            self.audio_base_decode_time =
                                                (abs_pts_ms * self.audio_sample_rate as u64) / 1000;
                                        }

                                        // Lock in the exact timestamp for this specific frame
                                        let dts = self.audio_base_decode_time;

                                        // THE AUDIO TIMELINE MAP
                                        let samples_per_frame = match self.audio_codec_id.as_str() {
                                            "A_AC3" | "A_EAC3" => 1536, // Dolby Digital
                                            "A_OPUS" => 960,            // Opus
                                            "A_DTS" => 512,             // DTS Core
                                            "A_TRUEHD" => 40,           // TrueHD
                                            "A_FLAC" => 4096,           // FLAC
                                            _ => 1024,                  // AAC & Fallback
                                        };

                                        // Advance the continuous clock for the NEXT frame
                                        self.audio_base_decode_time += samples_per_frame;

                                        // THE TRAFFIC COP
                                        if self.needs_transcode {
                                            self.raw_audio_queue.push(Frame {
                                                size: frame_data.len() as u32,
                                                data: frame_data,
                                                is_keyframe: true,
                                                dts: dts,
                                                ctts: 0,
                                            });
                                        } else {
                                            self.staged_audio_frames.push(Frame {
                                                size: frame_data.len() as u32,
                                                data: frame_data,
                                                is_keyframe: true,
                                                dts: dts,
                                                ctts: 0,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if found_cluster && !force_process {
            let tail = self.buffer[last_valid_idx..].to_vec();
            self.buffer = tail;
        } else if force_process {
            if !is_final && self.buffer.len() > 1024 * 1024 {
                let cutoff = self.buffer.len() - (1024 * 1024);
                let tail = self.buffer[cutoff..].to_vec();
                self.buffer = tail;
            } else {
                self.buffer.clear();
            }
        }

        self.staged_video_frames.len()
    }

    pub fn get_mp4_segment(&mut self) -> Vec<u8> {
        if self.staged_video_frames.is_empty() && self.staged_audio_frames.is_empty() {
            return Vec::new();
        }

        let mut moof = Vec::new();
        let header = start_box(&mut moof, b"moof");
        let mfhd = start_box(&mut moof, b"mfhd");
        moof.extend_from_slice(&[0; 4]);
        moof.extend_from_slice(&self.seq_number.to_be_bytes());
        end_box(&mut moof, mfhd);
        self.seq_number += 1;

        let mut video_offset_pos = 0;
        let mut audio_offset_pos = 0;

        if !self.staged_video_frames.is_empty() {
            let traf = start_box(&mut moof, b"traf");
            let tfhd = start_box(&mut moof, b"tfhd");
            moof.extend_from_slice(&[0x00, 0x02, 0x00, 0x00]);
            moof.extend_from_slice(&1u32.to_be_bytes());
            end_box(&mut moof, tfhd);

            let tfdt = start_box(&mut moof, b"tfdt");
            moof.extend_from_slice(&[0x01, 0, 0, 0]);
            moof.extend_from_slice(&self.staged_video_frames[0].dts.to_be_bytes());
            end_box(&mut moof, tfdt);

            let trun = start_box(&mut moof, b"trun");
            moof.extend_from_slice(&[0x01, 0x00, 0x0F, 0x01]);
            moof.extend_from_slice(&(self.staged_video_frames.len() as u32).to_be_bytes());

            video_offset_pos = moof.len();
            moof.extend_from_slice(&0i32.to_be_bytes());

            for f in &self.staged_video_frames {
                moof.extend_from_slice(&(self.frame_duration as u32).to_be_bytes());
                moof.extend_from_slice(&f.size.to_be_bytes());
                let flags: u32 = if f.is_keyframe {
                    0x02000000
                } else {
                    0x01010000
                };
                moof.extend_from_slice(&flags.to_be_bytes());
                moof.extend_from_slice(&f.ctts.to_be_bytes());
            }
            end_box(&mut moof, trun);
            end_box(&mut moof, traf);
        }

        if !self.staged_audio_frames.is_empty() {
            let traf = start_box(&mut moof, b"traf");
            let tfhd = start_box(&mut moof, b"tfhd");
            moof.extend_from_slice(&[0x00, 0x02, 0x00, 0x00]);
            moof.extend_from_slice(&2u32.to_be_bytes());
            end_box(&mut moof, tfhd);

            let tfdt = start_box(&mut moof, b"tfdt");
            moof.extend_from_slice(&[0x01, 0, 0, 0]);
            moof.extend_from_slice(&self.staged_audio_frames[0].dts.to_be_bytes());
            end_box(&mut moof, tfdt);

            let trun = start_box(&mut moof, b"trun");
            moof.extend_from_slice(&[0x00, 0x00, 0x03, 0x01]);
            moof.extend_from_slice(&(self.staged_audio_frames.len() as u32).to_be_bytes());

            audio_offset_pos = moof.len();
            moof.extend_from_slice(&0i32.to_be_bytes());

            let audio_frame_duration = 1024u32;

            for f in &self.staged_audio_frames {
                moof.extend_from_slice(&audio_frame_duration.to_be_bytes());
                moof.extend_from_slice(&f.size.to_be_bytes());
            }
            end_box(&mut moof, trun);
            end_box(&mut moof, traf);
        }

        end_box(&mut moof, header);

        let moof_size = moof.len();
        let total_v: u32 = self.staged_video_frames.iter().map(|f| f.size).sum();
        let total_a: u32 = self.staged_audio_frames.iter().map(|f| f.size).sum();

        if video_offset_pos > 0 {
            let offset = (moof_size + 8) as i32;
            let bytes = offset.to_be_bytes();
            moof[video_offset_pos..video_offset_pos + 4].copy_from_slice(&bytes);
        }

        if audio_offset_pos > 0 {
            let offset = (moof_size + 8 + total_v as usize) as i32;
            let bytes = offset.to_be_bytes();
            moof[audio_offset_pos..audio_offset_pos + 4].copy_from_slice(&bytes);
        }

        let mut mdat = Vec::new();
        write_box_header(&mut mdat, b"mdat", 8 + total_v + total_a);
        for f in &self.staged_video_frames {
            mdat.extend_from_slice(&f.data);
        }
        for f in &self.staged_audio_frames {
            mdat.extend_from_slice(&f.data);
        }

        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        self.staged_video_frames.clear();
        self.staged_audio_frames.clear();

        segment
    }
}

// --- FFI DEMUXER EXPORTS ---

#[no_mangle]
pub extern "C" fn demuxer_create(
    video_track_id: u64,
    audio_track_id: u64,
    width: u16,
    height: u16,
    duration_ms: f64,
    codec_id_ptr: *const c_char,
) -> *mut Demuxer {
    let codec_id = unsafe {
        if codec_id_ptr.is_null() {
            "V_MPEG4/ISO/AVC".to_string()
        } else {
            CStr::from_ptr(codec_id_ptr).to_string_lossy().into_owned()
        }
    };

    let demuxer = Demuxer::new(
        video_track_id,
        audio_track_id,
        width,
        height,
        duration_ms,
        codec_id,
    );
    Box::into_raw(Box::new(demuxer))
}

#[no_mangle]
pub extern "C" fn demuxer_destroy(ptr: *mut Demuxer) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(ptr);
    }
}

#[no_mangle]
pub extern "C" fn demuxer_reset(ptr: *mut Demuxer) {
    if ptr.is_null() {
        return;
    }
    let demuxer = unsafe { &mut *ptr };
    demuxer.reset();
}

#[no_mangle]
pub extern "C" fn demuxer_init(ptr: *mut Demuxer, chunk_ptr: *const u8, chunk_len: usize) -> u32 {
    let demuxer = unsafe { &mut *ptr };
    let chunk = unsafe { std::slice::from_raw_parts(chunk_ptr, chunk_len) };

    let init_segment = demuxer.init(chunk).unwrap_or_default();

    let mut boxed_slice = init_segment.into_boxed_slice();
    demuxer.last_segment_len = boxed_slice.len(); // Store length safely!
    let segment_ptr = boxed_slice.as_mut_ptr();
    std::mem::forget(boxed_slice);

    segment_ptr as u32 // Only return the 32-bit pointer!
}

#[no_mangle]
pub extern "C" fn demuxer_get_mp4_segment(ptr: *mut Demuxer) -> u32 {
    let demuxer = unsafe { &mut *ptr };
    let segment = demuxer.get_mp4_segment();

    let mut boxed_slice = segment.into_boxed_slice();
    demuxer.last_segment_len = boxed_slice.len(); // Store length safely!
    let segment_ptr = boxed_slice.as_mut_ptr();
    std::mem::forget(boxed_slice);

    segment_ptr as u32
}

// NEW: JS calls this to get the length!
#[no_mangle]
pub extern "C" fn demuxer_get_last_len(ptr: *mut Demuxer) -> u32 {
    let demuxer = unsafe { &mut *ptr };
    demuxer.last_segment_len as u32
}

#[no_mangle]
pub extern "C" fn demuxer_parse_chunk(
    ptr: *mut Demuxer,
    chunk_ptr: *const u8,
    chunk_len: usize,
    is_final: bool,
) -> usize {
    let demuxer = unsafe { &mut *ptr };
    let chunk = unsafe { std::slice::from_raw_parts(chunk_ptr, chunk_len) };
    demuxer.parse_chunk(chunk, is_final)
}

// UTILS
fn read_vint(data: &[u8]) -> (u64, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    let v = data[0];
    let len = if v & 0x80 != 0 {
        1
    } else if v & 0x40 != 0 {
        2
    } else if v & 0x20 != 0 {
        3
    } else {
        4
    };
    if len > 4 || data.len() < len {
        return (0, 0);
    }
    let mut value = (v & (0xFF >> len)) as u64;
    for i in 1..len {
        value = (value << 8) | (data[i] as u64);
    }
    (value, len)
}
fn write_box_header(w: &mut Vec<u8>, box_type: &[u8; 4], size: u32) {
    w.extend_from_slice(&size.to_be_bytes());
    w.extend_from_slice(box_type);
}
fn start_box(w: &mut Vec<u8>, box_type: &[u8; 4]) -> usize {
    let pos = w.len();
    w.extend_from_slice(&0u32.to_be_bytes());
    w.extend_from_slice(box_type);
    pos
}
fn end_box(w: &mut Vec<u8>, pos: usize) {
    let len = (w.len() - pos) as u32;
    let size_bytes = len.to_be_bytes();
    w[pos] = size_bytes[0];
    w[pos + 1] = size_bytes[1];
    w[pos + 2] = size_bytes[2];
    w[pos + 3] = size_bytes[3];
}
fn read_signed_vint(data: &[u8]) -> (i64, usize) {
    let (v, len) = read_vint(data);
    if len == 0 {
        return (0, 0);
    }
    let shift = 7 * len - 1;
    let zero_point = (1i64 << shift) - 1;
    ((v as i64) - zero_point, len)
}

fn parse_lacing(block_data: &[u8], header_len: usize) -> Vec<Vec<u8>> {
    let flags = block_data[header_len + 2];
    let lacing = (flags & 0x06) >> 1;
    let payload_offset = header_len + 3;

    if lacing == 0 || block_data.len() <= payload_offset {
        return vec![block_data[payload_offset..].to_vec()];
    }

    let payload = &block_data[payload_offset..];
    let num_frames = (payload[0] as usize) + 1;
    let mut pos = 1;
    let mut sizes = Vec::new();

    if lacing == 2 {
        let size = (payload.len() - 1) / num_frames;
        for _ in 0..num_frames {
            sizes.push(size);
        }
    } else if lacing == 1 {
        let mut total = 0;
        for _ in 0..(num_frames - 1) {
            let mut s = 0;
            while pos < payload.len() && payload[pos] == 255 {
                s += 255;
                pos += 1;
            }
            if pos < payload.len() {
                s += payload[pos] as usize;
                pos += 1;
            }
            sizes.push(s);
            total += s;
        }
        sizes.push(payload.len() - pos - total);
    } else if lacing == 3 {
        if pos >= payload.len() {
            return vec![payload.to_vec()];
        }

        let (v, len) = read_vint(&payload[pos..]);
        pos += len;
        sizes.push(v as usize);
        let mut total = v as usize;
        let mut last_size = v as i64;

        for _ in 1..(num_frames - 1) {
            if pos >= payload.len() {
                break;
            }

            let (diff, len) = read_signed_vint(&payload[pos..]);
            pos += len;
            last_size += diff;
            sizes.push(last_size as usize);
            total += last_size as usize;
        }
        if payload.len() >= pos + total {
            sizes.push(payload.len() - pos - total);
        } else {
            sizes.push(0);
        }
    }

    let mut frames = Vec::new();
    for s in sizes {
        if pos + s <= payload.len() {
            frames.push(payload[pos..pos + s].to_vec());
            pos += s;
        }
    }
    frames
}

// WRAPPERS FOR C

#[no_mangle]
pub extern "C" fn demuxer_get_last_audio_dts(ptr: *mut Demuxer) -> u64 {
    let demuxer = unsafe { &mut *ptr };
    demuxer.last_audio_dts
}

#[no_mangle]
pub extern "C" fn demuxer_decode_next_audio_frame(ptr: *mut Demuxer) -> i32 {
    let demuxer = unsafe { &mut *ptr };

    if demuxer.raw_audio_queue.is_empty() {
        return 0;
    }

    let frame = demuxer.raw_audio_queue.remove(0);

    // Save the exact timestamp so JS can tag the AAC frame
    demuxer.last_audio_dts = frame.dts;

    // LOG RUST HANDING OFF TO C
    //println!(
    //    "[Rust to C] Handing packet to C Decoder... Size: {} bytes",
    //    frame.data.len()
    //);

    // Hand it directly to the C Decoder
    let samples_produced = unsafe { decode_audio(frame.data.as_ptr(), frame.data.len() as i32) };

    samples_produced
}

#[no_mangle]
pub extern "C" fn demuxer_set_transcode_mode(ptr: *mut Demuxer, transcode: bool) {
    if ptr.is_null() {
        return;
    }
    let demuxer = unsafe { &mut *ptr };
    demuxer.needs_transcode = transcode;
}

// JS calls this when the Hardware Encoder finishes making an AAC chunk
#[no_mangle]
pub extern "C" fn demuxer_append_aac(
    ptr: *mut Demuxer,
    aac_ptr: *const u8,
    aac_len: usize,
    dts: u64,
) {
    let demuxer = unsafe { &mut *ptr };

    // Copy the AAC bytes from WebAssembly memory safely into Rust
    let aac_data = unsafe { std::slice::from_raw_parts(aac_ptr, aac_len) }.to_vec();

    // Put it in the final queue for the MP4
    demuxer.staged_audio_frames.push(Frame {
        size: aac_len as u32,
        data: aac_data,
        is_keyframe: true,
        dts: dts,
        ctts: 0,
    });
}

fn main() {}

//source ./emsdk_env.sh
//cd "C:\Users\manos\Documents\Projects\MKV Transcoder\streaming-engine"
//cargo build --target wasm32-unknown-emscripten --release

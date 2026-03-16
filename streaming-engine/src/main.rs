use serde::Serialize;
use std::ffi::CString;
use std::os::raw::c_char;
use webm_iterable::matroska_spec::MatroskaSpec;
use webm_iterable::WebmIterator;

extern "C" {
    fn init_audio_decoder(codec_id: i32, sample_rate: i32, channels: i32, extradata: *const u8, extradata_size: i32) -> i32;
    fn decode_audio_frame(input_data: *const u8, input_size: i32) -> i32;
    fn get_audio_buffer_ptr() -> *const f32;
    fn cleanup_audio_decoder();
}

#[no_mangle]
pub extern "C" fn start_ffmpeg_engine(codec_id: i32) -> i32 {
    unsafe { init_audio_decoder(codec_id, 48000, 2, std::ptr::null(), 0) }
}

#[no_mangle]
pub extern "C" fn decode_ffmpeg_chunk(chunk_ptr: *const u8, chunk_len: i32) -> i32 {
    unsafe { decode_audio_frame(chunk_ptr, chunk_len) }
}

#[no_mangle]
pub extern "C" fn get_ffmpeg_pcm_pointer() -> *const f32 {
    unsafe { get_audio_buffer_ptr() }
}

#[no_mangle]
pub extern "C" fn stop_ffmpeg_engine() {
    unsafe { cleanup_audio_decoder() }
}

// ---------------------------------------------------------
// 2. EMSCRIPTEN ZERO-COPY MEMORY ALLOCATORS
// ---------------------------------------------------------
#[no_mangle]
pub extern "C" fn alloc_memory(size: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn free_memory(ptr: *mut u8, size: usize) {
    unsafe {
        let _buf = Vec::from_raw_parts(ptr, 0, size);
    }
}

// ---------------------------------------------------------
// 3. MKV METADATA PARSING (JSON RETURNS)
// ---------------------------------------------------------
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

#[no_mangle]
pub extern "C" fn get_mkv_info_fast(ptr: *const u8, len: usize) -> *mut c_char {
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
        // --- NEW DOLBY CODECS ---
        else if codec == "A_AC3" {
            "ac-3".to_string()
        } else if codec == "A_EAC3" {
            "ec-3".to_string()
        }
        // This is Dolby Digital Plus / Atmos!
        else if codec == "A_TRUEHD" {
            "mlpa".to_string()
        } else if codec == "A_DTS" {
            "dtsc".to_string()
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
            language: t_lang.clone(),
        });
    }

    let final_duration = (duration_raw * timecode_scale as f64) / 1_000_000_000.0;
    let header = MkvHeader {
        duration: final_duration,
        tracks,
        cues_position: cues_pos,
    };

    // Convert to JSON String and return pointer
    let json_string = serde_json::to_string(&header).unwrap_or_else(|_| "{}".to_string());
    CString::new(json_string).unwrap().into_raw()
}

#[no_mangle]
pub extern "C" fn parse_cues(ptr: *const u8, len: usize) -> *mut c_char {
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
    CString::new(json_string).unwrap().into_raw()
}

// ---------------------------------------------------------
// 4. THE DEMUXER STATE MACHINE
// ---------------------------------------------------------
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
    current_cluster_time: u64,
    needs_sync: bool,

    // NEW: We store the output bytes here so JS can grab them easily
    last_output_buffer: Vec<u8>,
    needs_ffmpeg: bool,
    pcm_buffer: Vec<f32>,
}

#[no_mangle]
pub extern "C" fn demuxer_new(video_track_id: u64, audio_track_id: u64, width: u16, height: u16, codec_id_ptr: *const c_char) -> *mut Demuxer {
    let c_str = unsafe { std::ffi::CStr::from_ptr(codec_id_ptr) };
    let codec_id = c_str.to_str().unwrap_or("").to_string();

    let demuxer = Box::new(Demuxer {
        buffer: Vec::new(),
        video_track_id,
        audio_track_id,
        codec_id,
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
        current_cluster_time: 0,
        needs_sync: true,
        last_output_buffer: Vec::new(),
        needs_ffmpeg : false,
        pcm_buffer: Vec::new(),
    });
    Box::into_raw(demuxer)
}

// 🚨 NEW: DEDICATED FFI BRIDGE TO AVOID BIGINT MISALIGNMENT
#[no_mangle]
pub extern "C" fn demuxer_set_ffmpeg(demuxer_ptr: *mut Demuxer, needs_ffmpeg: i32) {
    let demuxer = unsafe { &mut *demuxer_ptr };
    demuxer.needs_ffmpeg = needs_ffmpeg > 0;
}

#[no_mangle]
pub extern "C" fn demuxer_reset(demuxer_ptr: *mut Demuxer) {
    let demuxer = unsafe { &mut *demuxer_ptr };
    demuxer.buffer.clear();
    demuxer.staged_video_frames.clear();
    demuxer.staged_audio_frames.clear();
    demuxer.base_decode_time = 0;
    demuxer.audio_base_decode_time = 0;
    demuxer.needs_sync = true;
    demuxer.pcm_buffer.clear(); // <-- NEW
}

#[no_mangle]
pub extern "C" fn demuxer_init(
    demuxer_ptr: *mut Demuxer,
    chunk_ptr: *const u8,
    chunk_len: usize,
) -> i32 {
    let demuxer = unsafe { &mut *demuxer_ptr };
    let chunk = unsafe { std::slice::from_raw_parts(chunk_ptr, chunk_len) };

    demuxer.buffer.extend_from_slice(chunk);
    let iterator = WebmIterator::new(&demuxer.buffer[..], &[]);
    let mut t_num = 0;
    let mut t_dur_ns = 0;
    let mut a_codec = String::new();

    for element in iterator {
        if let Ok(MatroskaSpec::Cluster(_)) = element {
            break;
        }
        match element {
            Ok(MatroskaSpec::TrackEntry(_)) => {
                if t_num == demuxer.video_track_id && t_dur_ns > 0 {
                    demuxer.frame_duration = (t_dur_ns * 90000) / 1_000_000_000;
                }
                t_num = 0;
                t_dur_ns = 0;
            }
            Ok(MatroskaSpec::TrackNumber(num)) => t_num = num,
            Ok(MatroskaSpec::CodecID(id)) => {
                if t_num == demuxer.audio_track_id {
                    a_codec = id;
                }
            }
            Ok(MatroskaSpec::CodecPrivate(data)) => {
                if t_num == demuxer.video_track_id {
                    demuxer.codec_private = data.clone();
                } else if t_num == demuxer.audio_track_id {
                    demuxer.audio_private = data.clone();
                }
            }
            Ok(MatroskaSpec::DefaultDuration(d)) => t_dur_ns = d,
            Ok(MatroskaSpec::SamplingFrequency(freq)) => {
                if t_num == demuxer.audio_track_id {
                    demuxer.audio_sample_rate = freq as u32;
                }
            }
            Ok(MatroskaSpec::Channels(ch)) => {
                if t_num == demuxer.audio_track_id {
                    demuxer.audio_channels = ch as u16;
                }
            }
            _ => {}
        }
    }

    if demuxer.codec_private.is_empty() {
        return -1;
    } // Error

    if demuxer.codec_id == "V_MPEG4/ISO/AVC"
        && demuxer.codec_private.len() > 3
        && demuxer.codec_private[1] == 0x53
    {
        demuxer.codec_private[1] = 0x64;
    }

    // --- NEW FFMPEG BOOTUP ---
    if demuxer.needs_ffmpeg {
        let ffmpeg_codec = match a_codec.as_str() { "A_EAC3" => 86057, "A_AC3" => 86019, "A_TRUEHD" => 86060, "A_DTS" => 86020, _ => 86019 };
        
        let extradata_ptr = if demuxer.audio_private.is_empty() { std::ptr::null() } else { demuxer.audio_private.as_ptr() };
        unsafe { init_audio_decoder(ffmpeg_codec, demuxer.audio_sample_rate as i32, demuxer.audio_channels as i32, extradata_ptr, demuxer.audio_private.len() as i32) };
    }

    demuxer.initialized = true;
    demuxer.buffer.clear();

    // THE AUDIO HIDE TRICK: If FFmpeg is running, tell the MP4 builder the audio ID is 0!
    let mp4_audio_id = if demuxer.needs_ffmpeg {
        0
    } else {
        demuxer.audio_track_id
    };

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

    let next_track_id = if mp4_audio_id > 0 { 3u32 } else { 2u32 }; // <-- CHANGED HERE
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

    if mp4_audio_id > 0 { // <--- THIS IS THE ONLY CHANGE
        let trex_aud = start_box(&mut mp4, b"trex"); mp4.extend_from_slice(&[0; 4]); mp4.extend_from_slice(&2u32.to_be_bytes()); mp4.extend_from_slice(&1u32.to_be_bytes()); mp4.extend_from_slice(&0u32.to_be_bytes()); mp4.extend_from_slice(&0u32.to_be_bytes()); mp4.extend_from_slice(&0u32.to_be_bytes()); end_box(&mut mp4, trex_aud);
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
        0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00,
    ]);
    mp4.extend_from_slice(&((demuxer.width as u32) << 16).to_be_bytes());
    mp4.extend_from_slice(&((demuxer.height as u32) << 16).to_be_bytes());
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

    let is_hevc = demuxer.codec_id == "V_MPEGH/ISO/HEVC";
    let sample_entry_tag = if is_hevc { b"hev1" } else { b"avc1" };
    let config_box_tag = if is_hevc { b"hvcC" } else { b"avcC" };

    let sample_entry = start_box(&mut mp4, sample_entry_tag);
    mp4.extend_from_slice(&[0; 6]);
    mp4.extend_from_slice(&1u16.to_be_bytes());
    mp4.extend_from_slice(&[0; 16]);
    mp4.extend_from_slice(&demuxer.width.to_be_bytes());
    mp4.extend_from_slice(&demuxer.height.to_be_bytes());
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
        (8 + demuxer.codec_private.len()) as u32,
    );
    mp4.extend_from_slice(&demuxer.codec_private);
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

    if mp4_audio_id > 0 && !demuxer.audio_private.is_empty() {
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
            0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00, 0,
            0, 0, 0, 0, 0, 0, 0,
        ]);
        end_box(&mut mp4, a_tkhd);

        let a_mdia = start_box(&mut mp4, b"mdia");
        let a_mdhd = start_box(&mut mp4, b"mdhd");
        mp4.extend_from_slice(&[0; 12]);
        mp4.extend_from_slice(&demuxer.audio_sample_rate.to_be_bytes());
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
        mp4.extend_from_slice(&demuxer.audio_channels.to_be_bytes());
        mp4.extend_from_slice(&16u16.to_be_bytes());
        mp4.extend_from_slice(&0u16.to_be_bytes());
        mp4.extend_from_slice(&0u16.to_be_bytes());
        mp4.extend_from_slice(&(demuxer.audio_sample_rate << 16).to_be_bytes());

        let esds = start_box(&mut mp4, b"esds");
        mp4.extend_from_slice(&[0; 4]);

        let audio_private_len = demuxer.audio_private.len() as u8;
        let dec_specific_info_len = audio_private_len;
        let dec_config_len = 13 + 2 + dec_specific_info_len;
        let es_desc_len = 3 + 2 + dec_config_len + 3;

        mp4.extend_from_slice(&[0x03, es_desc_len]);
        mp4.extend_from_slice(&[0x00, 0x00, 0x00]);

        mp4.extend_from_slice(&[0x04, dec_config_len]);
        mp4.extend_from_slice(&[0x40, 0x15]);
        mp4.extend_from_slice(&[0x00, 0x00, 0x00]);
        mp4.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        mp4.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        mp4.extend_from_slice(&[0x05, dec_specific_info_len]);
        mp4.extend_from_slice(&demuxer.audio_private);

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

    // Save the bytes and return 1 (Success)
    demuxer.last_output_buffer = mp4;
    1
}

#[no_mangle]
pub extern "C" fn demuxer_parse_chunk(
    demuxer_ptr: *mut Demuxer,
    chunk_ptr: *const u8,
    chunk_len: usize,
    is_final: bool,
) -> usize {
    let demuxer = unsafe { &mut *demuxer_ptr };
    let chunk = unsafe { std::slice::from_raw_parts(chunk_ptr, chunk_len) };

    if !demuxer.initialized {
        return 0;
    }

    demuxer.buffer.extend_from_slice(chunk);

    if demuxer.needs_sync {
        let mut sync_idx = None;
        for i in 0..demuxer.buffer.len().saturating_sub(3) {
            if demuxer.buffer[i] == 0x1F
                && demuxer.buffer[i + 1] == 0x43
                && demuxer.buffer[i + 2] == 0xB6
                && demuxer.buffer[i + 3] == 0x75
            {
                sync_idx = Some(i);
                break;
            }
        }
        match sync_idx {
            Some(idx) => {
                demuxer.buffer.drain(0..idx);
                demuxer.needs_sync = false;
            }
            None => {
                if demuxer.buffer.len() > 3 {
                    let keep = demuxer.buffer.len() - 3;
                    demuxer.buffer.drain(0..keep);
                }
                return 0;
            }
        }
    }

    let mut last_valid_idx = 0;
    let mut found_cluster = false;

    if demuxer.buffer.len() > 10 {
        for i in (0..demuxer.buffer.len() - 4).rev() {
            if demuxer.buffer[i] == 0x1F
                && demuxer.buffer[i + 1] == 0x43
                && demuxer.buffer[i + 2] == 0xB6
                && demuxer.buffer[i + 3] == 0x75
            {
                last_valid_idx = i;
                found_cluster = true;
                break;
            }
        }
    }

    let process_limit = if found_cluster { last_valid_idx } else { 0 };
    let force_process = is_final || demuxer.buffer.len() > 60 * 1024 * 1024;
    let actual_limit = if force_process {
        demuxer.buffer.len()
    } else {
        process_limit
    };

    if actual_limit > 0 {
        let iterator = WebmIterator::new(&demuxer.buffer[0..actual_limit], &[]);
        let mut safety = 0;

        for element in iterator {
            safety += 1;
            if safety > 5_000_000 {
                break;
            }

            match element {
                Ok(MatroskaSpec::Timestamp(tc)) => demuxer.current_cluster_time = tc,
                Ok(MatroskaSpec::SimpleBlock(block_data)) => {
                    let (track_id, header_len) = read_vint(&block_data);
                    if block_data.len() > header_len + 3 {
                        let offset = header_len + 3;
                        let size = (block_data.len() - offset) as u32;
                        if size > 0 {
                            let t_bytes = [block_data[header_len], block_data[header_len + 1]];
                            let rel_time = i16::from_be_bytes(t_bytes);
                            let abs_pts_ms =
                                (demuxer.current_cluster_time as i64 + rel_time as i64) as u64;

                            if track_id == demuxer.video_track_id {
                                let is_keyframe = (block_data[header_len + 2] & 0x80) == 0x80;
                                let pts = abs_pts_ms * 90;

                                if demuxer.base_decode_time > 0 {
                                    let gap = pts as i64 - demuxer.base_decode_time as i64;
                                    if gap > 45000 {
                                        demuxer.base_decode_time = pts - demuxer.frame_duration;
                                    }
                                } else {
                                    demuxer.base_decode_time = pts;
                                }

                                let dts = demuxer.base_decode_time;
                                demuxer.base_decode_time += demuxer.frame_duration;
                                let ctts = (pts as i64 - dts as i64) as i32;

                                demuxer.staged_video_frames.push(Frame {
                                    size,
                                    data: block_data[offset..].to_vec(),
                                    is_keyframe,
                                    dts,
                                    ctts,
                                });
                            }
                            else if track_id == demuxer.audio_track_id {
                                let frames = parse_lacing(&block_data, header_len);
                                
                                // --- THE HIGHWAY SPLIT ---
                                if demuxer.needs_ffmpeg {
                                    for frame_data in frames {
                                        let samples = unsafe { decode_audio_frame(frame_data.as_ptr(), frame_data.len() as i32) };
                                        if samples > 0 {
                                            let ptr = unsafe { get_audio_buffer_ptr() };
                                            let slice = unsafe { std::slice::from_raw_parts(ptr, (samples * 2) as usize) };
                                            demuxer.pcm_buffer.extend_from_slice(slice);
                                        }
                                    }
                                } else {
                                    // AAC Native Highway
                                    let audio_frame_duration = 1024u64;
                                    for frame_data in frames {
                                        let expected_ms = if demuxer.audio_sample_rate > 0 { (demuxer.audio_base_decode_time * 1000) / demuxer.audio_sample_rate as u64 } else { 0 };
                                        let gap = (abs_pts_ms as i64) - (expected_ms as i64);
                                        
                                        if demuxer.audio_base_decode_time == 0 || gap > 500 || gap < -500 {
                                            demuxer.audio_base_decode_time = (abs_pts_ms * demuxer.audio_sample_rate as u64) / 1000;
                                        }
                                        
                                        let dts = demuxer.audio_base_decode_time;
                                        demuxer.audio_base_decode_time += audio_frame_duration;
                                        demuxer.staged_audio_frames.push(Frame { size: frame_data.len() as u32, data: frame_data, is_keyframe: true, dts, ctts: 0 });
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
        let tail = demuxer.buffer[last_valid_idx..].to_vec();
        demuxer.buffer = tail;
    } else if force_process {
        if !is_final && demuxer.buffer.len() > 1024 * 1024 {
            let cutoff = demuxer.buffer.len() - (1024 * 1024);
            let tail = demuxer.buffer[cutoff..].to_vec();
            demuxer.buffer = tail;
        } else {
            demuxer.buffer.clear();
        }
    }

    demuxer.staged_video_frames.len()
}

#[no_mangle]
pub extern "C" fn demuxer_get_mp4_segment(demuxer_ptr: *mut Demuxer) -> i32 {
    let demuxer = unsafe { &mut *demuxer_ptr };

    if demuxer.staged_video_frames.is_empty() && demuxer.staged_audio_frames.is_empty() {
        demuxer.last_output_buffer.clear();
        return 0;
    }

    let mut moof = Vec::new();
    let header = start_box(&mut moof, b"moof");
    let mfhd = start_box(&mut moof, b"mfhd");
    moof.extend_from_slice(&[0; 4]);
    moof.extend_from_slice(&demuxer.seq_number.to_be_bytes());
    end_box(&mut moof, mfhd);
    demuxer.seq_number += 1;

    let mut video_offset_pos = 0;
    let mut audio_offset_pos = 0;

    if !demuxer.staged_video_frames.is_empty() {
        let traf = start_box(&mut moof, b"traf");
        let tfhd = start_box(&mut moof, b"tfhd");
        moof.extend_from_slice(&[0x00, 0x02, 0x00, 0x00]);
        moof.extend_from_slice(&1u32.to_be_bytes());
        end_box(&mut moof, tfhd);

        let tfdt = start_box(&mut moof, b"tfdt");
        moof.extend_from_slice(&[0x01, 0, 0, 0]);
        moof.extend_from_slice(&demuxer.staged_video_frames[0].dts.to_be_bytes());
        end_box(&mut moof, tfdt);

        let trun = start_box(&mut moof, b"trun");
        moof.extend_from_slice(&[0x01, 0x00, 0x0F, 0x01]);
        moof.extend_from_slice(&(demuxer.staged_video_frames.len() as u32).to_be_bytes());

        video_offset_pos = moof.len();
        moof.extend_from_slice(&0i32.to_be_bytes());

        for f in &demuxer.staged_video_frames {
            moof.extend_from_slice(&(demuxer.frame_duration as u32).to_be_bytes());
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

    if !demuxer.staged_audio_frames.is_empty() {
        let traf = start_box(&mut moof, b"traf");
        let tfhd = start_box(&mut moof, b"tfhd");
        moof.extend_from_slice(&[0x00, 0x02, 0x00, 0x00]);
        moof.extend_from_slice(&2u32.to_be_bytes());
        end_box(&mut moof, tfhd);

        let tfdt = start_box(&mut moof, b"tfdt");
        moof.extend_from_slice(&[0x01, 0, 0, 0]);
        moof.extend_from_slice(&demuxer.staged_audio_frames[0].dts.to_be_bytes());
        end_box(&mut moof, tfdt);

        let trun = start_box(&mut moof, b"trun");
        moof.extend_from_slice(&[0x00, 0x00, 0x03, 0x01]);
        moof.extend_from_slice(&(demuxer.staged_audio_frames.len() as u32).to_be_bytes());

        audio_offset_pos = moof.len();
        moof.extend_from_slice(&0i32.to_be_bytes());

        let audio_frame_duration = 1024u32;

        for f in &demuxer.staged_audio_frames {
            moof.extend_from_slice(&audio_frame_duration.to_be_bytes());
            moof.extend_from_slice(&f.size.to_be_bytes());
        }
        end_box(&mut moof, trun);
        end_box(&mut moof, traf);
    }

    end_box(&mut moof, header);

    let moof_size = moof.len();
    let total_v: u32 = demuxer.staged_video_frames.iter().map(|f| f.size).sum();
    let total_a: u32 = demuxer.staged_audio_frames.iter().map(|f| f.size).sum();

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
    for f in &demuxer.staged_video_frames {
        mdat.extend_from_slice(&f.data);
    }
    for f in &demuxer.staged_audio_frames {
        mdat.extend_from_slice(&f.data);
    }

    let mut segment = moof;
    segment.extend_from_slice(&mdat);
    demuxer.staged_video_frames.clear();
    demuxer.staged_audio_frames.clear();

    // Save to our buffer and return 1 (Success)
    demuxer.last_output_buffer = segment;
    1
}

// ---------------------------------------------------------
// NEW PCM FETCHERS FOR JAVASCRIPT
// ---------------------------------------------------------
#[no_mangle] 
pub extern "C" fn demuxer_get_pcm_ptr(demuxer_ptr: *mut Demuxer) -> *const f32 { 
    unsafe { &mut *demuxer_ptr }.pcm_buffer.as_ptr() 
}

#[no_mangle] 
pub extern "C" fn demuxer_get_pcm_len(demuxer_ptr: *mut Demuxer) -> usize { 
    unsafe { &mut *demuxer_ptr }.pcm_buffer.len() 
}

#[no_mangle] 
pub extern "C" fn demuxer_clear_pcm(demuxer_ptr: *mut Demuxer) { 
    unsafe { &mut *demuxer_ptr }.pcm_buffer.clear(); 
}

#[no_mangle]
pub extern "C" fn demuxer_get_output_ptr(demuxer_ptr: *mut Demuxer) -> *const u8 {
    let demuxer = unsafe { &mut *demuxer_ptr };
    demuxer.last_output_buffer.as_ptr()
}

#[no_mangle]
pub extern "C" fn demuxer_get_output_len(demuxer_ptr: *mut Demuxer) -> usize {
    let demuxer = unsafe { &mut *demuxer_ptr };
    demuxer.last_output_buffer.len()
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
        let (v, len) = read_vint(&payload[pos..]);
        pos += len;
        sizes.push(v as usize);
        let mut total = v as usize;
        let mut last_size = v as i64;

        for _ in 1..(num_frames - 1) {
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

fn main() {
    // Dummy main to trick Cargo into building an executable
}

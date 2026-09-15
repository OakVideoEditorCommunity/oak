// Temporary diagnostic: render a media file's audio exactly like the
// playback prefetch does (one sequence frame per chunk through
// `eval::render_audio_samples`) and write the result as a PCM s16 WAV,
// so render-side artifacts can be analyzed offline.
//
// Usage: render_audio_wav <media> <stream_index> <start_sec> <end_sec> <out.wav> [fps]

use oak_core::{Rational, TimeRange};
use oak_render::eval::render_audio_samples;
use oak_render::ticket::{AudioTicketParams, MontageClip, TicketPayload};

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.len() < 6 {
		eprintln!(
			"usage: render_audio_wav <media> <stream_index> <start_sec> <end_sec> <out.wav> [fps]"
		);
		std::process::exit(64);
	}
	let media = &args[1];
	let stream_index: i32 = args[2].parse().unwrap();
	let start_sec: f64 = args[3].parse().unwrap();
	let end_sec: f64 = args[4].parse().unwrap();
	let out_path = &args[5];
	let fps: i64 = if args.len() > 6 {
		args[6].parse().unwrap()
	} else {
		25
	};

	let sample_rate = 48000i32;
	let channel_layout = 0x3u64; // stereo
	let channels = 2usize;

	let start_frame = (start_sec * fps as f64).round() as i64;
	let end_frame = (end_sec * fps as f64).round() as i64;

	let montage = vec![MontageClip {
		filename: media.clone(),
		stream_index,
		in_time: Rational::new(start_frame, fps),
		out_time: Rational::new(end_frame, fps),
		media_in: Rational::new(0, 1),
		gain: 1.0,
		effects: Vec::new(),
	}];

	let mut pcm: Vec<i16> = Vec::new();
	for f in start_frame..end_frame {
		let range = TimeRange::new(Rational::new(f, fps), Rational::new(f + 1, fps));
		let params = AudioTicketParams {
			viewer: 1,
			range,
			sample_rate,
			channel_layout,
			montage: montage.clone(),
		};
		match render_audio_samples(&params) {
			Ok(TicketPayload::Audio(samples)) => {
				for s in &samples.samples {
					pcm.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
				}
			}
			other => {
				eprintln!("frame {f}: render failed: {:?}", other.is_err());
				let frames = ((sample_rate as f64) / fps as f64).round() as usize;
				pcm.extend(std::iter::repeat_n(0i16, frames * channels));
			}
		}
	}

	// Minimal 16-bit PCM WAV.
	let data_len = (pcm.len() * 2) as u32;
	let mut w = Vec::with_capacity(44 + data_len as usize);
	w.extend_from_slice(b"RIFF");
	w.extend_from_slice(&(36 + data_len).to_le_bytes());
	w.extend_from_slice(b"WAVEfmt ");
	w.extend_from_slice(&16u32.to_le_bytes());
	w.extend_from_slice(&1u16.to_le_bytes()); // PCM
	w.extend_from_slice(&(channels as u16).to_le_bytes());
	w.extend_from_slice(&(sample_rate as u32).to_le_bytes());
	w.extend_from_slice(&(sample_rate as u32 * channels as u32 * 2).to_le_bytes());
	w.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
	w.extend_from_slice(&16u16.to_le_bytes());
	w.extend_from_slice(b"data");
	w.extend_from_slice(&data_len.to_le_bytes());
	for s in &pcm {
		w.extend_from_slice(&s.to_le_bytes());
	}
	std::fs::write(out_path, &w).expect("write wav");
	println!(
		"wrote {} samples ({} ch @ {} Hz) to {}",
		pcm.len() / channels,
		channels,
		sample_rate,
		out_path
	);
}

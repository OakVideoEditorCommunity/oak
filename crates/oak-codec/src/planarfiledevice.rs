// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! `olive::PlanarFileDevice` — planar (per-channel) file read/write.
//!
//! Mirrors `src/codec/src/planarfiledevice.h`. Reads/writes one FILE* per
//! channel so multi-channel audio is stored as one planar file per channel.
//! `FILE*` implementation (the QIODevice-based original was replaced).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Open mode (replaces QIODevice::OpenMode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum OpenMode {
	/// Read-only.
	ReadOnly = 0,
	/// Write-only.
	WriteOnly = 1,
}

/// `olive::PlanarFileDevice` — one file per channel.
pub struct PlanarFileDevice {
	/// Open file handles, one per channel.
	files: Vec<File>,
	/// Open mode.
	mode: OpenMode,
}

impl PlanarFileDevice {
	/// New, closed device.
	pub fn new() -> Self {
		PlanarFileDevice {
			files: Vec::new(),
			mode: OpenMode::ReadOnly,
		}
	}

	/// Whether the device is open.
	pub fn is_open(&self) -> bool {
		!self.files.is_empty()
	}

	/// Open `filenames` (one per channel) in `mode`. Returns false if already
	/// open or any file could not be opened (closing any opened so far).
	pub fn open(&mut self, filenames: &[PathBuf], mode: OpenMode) -> bool {
		if self.is_open() {
			return false;
		}

		let mut opened = Vec::with_capacity(filenames.len());
		for name in filenames {
			let mut opt = OpenOptions::new();
			let f = match mode {
				OpenMode::ReadOnly => opt.read(true).open(name),
				OpenMode::WriteOnly => opt.create(true).write(true).truncate(true).open(name),
			};
			match f {
				Ok(f) => opened.push(f),
				Err(_) => {
					// Roll back: close anything opened so far and report failure.
					self.files = opened;
					self.close();
					return false;
				}
			}
		}

		self.files = opened;
		self.mode = mode;
		true
	}

	/// Read `bytes_per_channel` bytes from each channel (at the current file
	/// position) into `data[i][offset..]`. Returns bytes read per channel, or
	/// -1 if closed or a buffer is too small.
	pub fn read(&mut self, data: &mut [&mut [u8]], bytes_per_channel: i64, offset: i64) -> i64 {
		if !self.is_open() {
			return -1;
		}
		let bytes = bytes_per_channel as usize;
		let off = offset as usize;
		let mut ret = -1i64;
		for (i, f) in self.files.iter_mut().enumerate() {
			let buf = match data.get_mut(i) {
				Some(b) if b.len() >= off + bytes => &mut b[off..off + bytes],
				_ => return -1,
			};
			ret = f.read(buf).unwrap_or(0) as i64;
		}
		ret
	}

	/// Write `bytes_per_channel` bytes to each channel from `data[i][offset..]`.
	/// Returns bytes written per channel, or -1.
	pub fn write(&mut self, data: &[&[u8]], bytes_per_channel: i64, offset: i64) -> i64 {
		if !self.is_open() {
			return -1;
		}
		let bytes = bytes_per_channel as usize;
		let off = offset as usize;
		let mut ret = -1i64;
		for (i, f) in self.files.iter_mut().enumerate() {
			let buf = match data.get(i) {
				Some(b) if b.len() >= off + bytes => &b[off..off + bytes],
				_ => return -1,
			};
			ret = f.write(buf).unwrap_or(0) as i64;
		}
		ret
	}

	/// Total size in bytes of one channel (from the first open file).
	pub fn size(&self) -> i64 {
		if self.is_open() {
			if let Ok(meta) = self.files[0].metadata() {
				return meta.len() as i64;
			}
		}
		0
	}

	/// Seek all channels to `pos`.
	pub fn seek(&mut self, pos: i64) -> bool {
		let mut ok = true;
		for f in self.files.iter_mut() {
			ok = f.seek(SeekFrom::Start(pos as u64)).is_ok() && ok;
		}
		ok
	}

	/// Close all channels.
	pub fn close(&mut self) {
		self.files.clear();
	}
}

impl Default for PlanarFileDevice {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn temp_dir(name: &str) -> PathBuf {
		let dir =
			std::env::temp_dir().join(format!("oakcodec_planar_{}_{}", name, std::process::id()));
		let _ = std::fs::create_dir_all(&dir);
		dir
	}

	#[test]
	fn new_device_is_closed() {
		let mut d = PlanarFileDevice::new();
		assert!(!d.is_open());
		assert_eq!(d.size(), 0);
		assert_eq!(d.read(&mut [&mut [0u8; 4]], 4, 0), -1);
		assert_eq!(d.write(&[&[0u8; 4]], 4, 0), -1);
	}

	#[test]
	fn write_then_read_roundtrip() {
		let dir = temp_dir("rw");
		let names: Vec<PathBuf> = (0..2).map(|i| dir.join(format!("ch{}.pcm", i))).collect();

		let mut d = PlanarFileDevice::new();
		assert!(d.open(&names, OpenMode::WriteOnly));
		assert!(d.is_open());
		assert_eq!(d.mode, OpenMode::WriteOnly);

		// Two channels, 4 bytes each.
		let ch0 = [1u8, 2, 3, 4];
		let ch1 = [9u8, 8, 7, 6];
		assert_eq!(d.write(&[&ch0, &ch1], 4, 0), 4);
		assert_eq!(d.size(), 4);
		d.close();
		assert!(!d.is_open());

		// A "wb" handle cannot be read back (matching the C++ fopen mode);
		// reopen read-only to verify the written bytes.
		let mut d = PlanarFileDevice::new();
		assert!(d.open(&names, OpenMode::ReadOnly));
		assert_eq!(d.size(), 4);
		let mut out0 = [0u8; 8];
		let mut out1 = [0u8; 8];
		assert_eq!(d.read(&mut [&mut out0[..], &mut out1[..]], 4, 0), 4);
		assert_eq!(&out0[..4], &ch0);
		assert_eq!(&out1[..4], &ch1);
		d.close();
		assert!(!d.is_open());
	}

	#[test]
	fn offset_writes_and_reads() {
		let dir = temp_dir("off");
		let names = vec![dir.join("ch.pcm")];
		let mut d = PlanarFileDevice::new();
		assert!(d.open(&names, OpenMode::WriteOnly));

		// The offset is a *buffer* offset (C++ `data[i] + offset`); file
		// offsets go through `seek`. Seek to 4 then write 4 bytes, leaving
		// a 4-byte gap.
		let data = [7u8, 7, 7, 7];
		assert!(d.seek(4));
		assert_eq!(d.write(&[&data], 4, 0), 4);
		assert_eq!(d.size(), 8);

		d.close();
		let mut d = PlanarFileDevice::new();
		assert!(d.open(&names, OpenMode::ReadOnly));
		let mut buf = [0u8; 8];
		assert!(d.seek(0));
		assert_eq!(d.read(&mut [&mut buf[..]], 8, 0), 8);
		assert_eq!(&buf[..4], &[0, 0, 0, 0]);
		assert_eq!(&buf[4..], &data);
	}

	#[test]
	fn open_rollback_on_missing_file_and_double_open() {
		let dir = temp_dir("roll");
		let ok = dir.join("ok.pcm");
		let missing = dir.join("missing.pcm");
		let mut d = PlanarFileDevice::new();
		assert!(!d.open(&[ok.clone(), missing], OpenMode::ReadOnly));
		assert!(!d.is_open());

		// Already open -> refuse.
		assert!(d.open(std::slice::from_ref(&ok), OpenMode::WriteOnly));
		assert!(!d.open(&[ok], OpenMode::WriteOnly));
	}

	#[test]
	fn read_write_reject_small_buffers() {
		let dir = temp_dir("small");
		let names = vec![dir.join("ch.pcm")];
		let mut d = PlanarFileDevice::new();
		assert!(d.open(&names, OpenMode::WriteOnly));
		let data = [1u8; 8];
		assert_eq!(d.write(&[&data], 8, 0), 8);

		// Buffer too small for the requested bytes.
		let mut small = [0u8; 4];
		assert_eq!(d.write(&[&small], 8, 0), -1);
		assert!(d.seek(0));
		assert_eq!(d.read(&mut [&mut small[..]], 8, 0), -1);
	}
}

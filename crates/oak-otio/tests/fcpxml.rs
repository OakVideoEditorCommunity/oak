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

//! Integration tests for the FCPXML layer: parsing a synthetic FCPXML
//! document, round-tripping the model through the writer and reader, NTSC
//! frame accuracy, error paths (corrupt XML, missing resources, unknown
//! version) and lenient handling of unknown elements.

use oak_core::Rational;
use oak_otio::{
	Clip, Composable, ExternalReference, MediaReference, RationalTime, Timeline, Track,
};

/// The NTSC video rate 30000/1001 (~29.97 fps) as an exact rational.
const NTSC_RATE: f64 = 30000.0 / 1001.0;

/// A realistic FCPXML 1.10 document: a 30 fps spine with a clip, a
/// centered transition, a disabled clip, a gap and a title, plus a
/// secondary audio track with NTSC (29.97 fps) material.
fn synthetic_fcpxml() -> String {
	r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.10">
	<resources>
		<format id="r1" name="FFVideoFormat1080p30" frameDuration="100/3000s" width="1920" height="1080"/>
		<format id="r2" name="FFVideoFormatRate29_97i" frameDuration="1001/30000s" width="1920" height="1080"/>
		<asset id="r3" name="Clip A.mov" src="file:///tmp/Clip A.mov" format="r1" duration="12000/3000s" hasVideo="1" hasAudio="1"/>
		<asset id="r4" name="Ambience.wav" src="file:///tmp/Ambience.wav" format="r2" duration="3003/30000s" hasVideo="0" hasAudio="1"/>
		<effect id="r5" name="Custom" uid="9C61DDC9-1111-2222-3333-444455556666"/>
	</resources>
	<library>
		<event name="My Event">
			<project name="My Project">
				<sequence format="r1" tcStart="3600/3000s" tcFormat="NDF" duration="2220/3000s" audioLayout="stereo" audioRate="48000">
					<spine>
						<asset-clip name="Clip A" ref="r3" offset="0s" duration="1200/3000s" start="2400/3000s" format="r1"/>
						<transition name="Cross Dissolve" offset="1140/3000s" duration="120/3000s"/>
						<asset-clip name="Clip B" ref="r3" offset="1260/3000s" duration="600/3000s" start="0s" enabled="0"/>
						<gap name="Tail" offset="1860/3000s" duration="60/3000s"/>
						<title name="Hello" offset="1920/3000s" duration="300/3000s"/>
					</spine>
					<audio>
						<asset-clip name="Ambience" ref="r4" offset="0s" duration="3003/30000s" start="0s" format="r2"/>
					</audio>
				</sequence>
			</project>
		</event>
	</library>
</fcpxml>
"#
	.to_string()
}

/// Parse the synthetic document and return its single timeline.
fn parse_synthetic() -> Timeline {
	let timelines =
		oak_otio::from_fcpxml_string(&synthetic_fcpxml()).expect("parse synthetic fcpxml");
	assert_eq!(timelines.len(), 1);
	timelines.into_iter().next().unwrap()
}

/// Assert a RationalTime is within tolerance of (value, rate).
fn assert_time(rt: RationalTime, value: f64, rate: f64) {
	assert!(
		(rt.value() - value).abs() < 1e-9,
		"value {} != {value}",
		rt.value()
	);
	assert!(
		(rt.rate() - rate).abs() < 1e-9,
		"rate {} != {rate}",
		rt.rate()
	);
}

#[test]
fn parse_synthetic_document() {
	let timeline = parse_synthetic();

	assert_eq!(timeline.name(), "My Project");
	let gst = timeline
		.global_start_time()
		.expect("tcStart maps to global start");
	assert_time(gst, 36.0, 30.0);

	// Interchange hints survive in the timeline metadata.
	let fcpx = timeline.metadata().get("fcpxml").expect("fcpxml metadata");
	assert_eq!(fcpx["version"], "1.10");
	assert_eq!(fcpx["tcFormat"], "NDF");
	assert_eq!(fcpx["audioLayout"], "stereo");
	assert_eq!(fcpx["audioRate"], "48000");
	assert_eq!(fcpx["event"], "My Event");
	assert_eq!(fcpx["project"], "My Project");

	// One video track (spine) and one audio track.
	let children = timeline.tracks().children();
	assert_eq!(children.len(), 2);
	let video = children[0].as_track().expect("first track is a Track");
	assert_eq!(video.kind(), "Video");
	let audio = children[1].as_track().expect("second track is a Track");
	assert_eq!(audio.kind(), "Audio");

	// Spine: clip, transition, clip, gap, title-as-gap.
	assert_eq!(video.children().len(), 5);
	let clip_a = video.children()[0]
		.as_clip()
		.expect("spine child 0 is a Clip");
	assert_eq!(clip_a.name(), "Clip A");
	assert!(clip_a.enabled());
	let range = clip_a.source_range().expect("Clip A has a source range");
	assert_time(range.start_time(), 24.0, 30.0);
	assert_time(range.duration(), 12.0, 30.0);
	assert_eq!(range.start_time().to_rational(), Rational::new(4, 5));
	assert_eq!(range.duration().to_rational(), Rational::new(2, 5));
	let external = match clip_a.media_reference().expect("Clip A resolves media") {
		MediaReference::ExternalReference(e) => e,
		other => panic!("expected ExternalReference, got {}", other.schema_name()),
	};
	assert_eq!(external.target_url(), "file:///tmp/Clip A.mov");
	let available = external.available_range().expect("asset duration");
	assert_time(available.start_time(), 0.0, 30.0);
	assert_time(available.duration(), 120.0, 30.0);

	// Centered transition: in_offset == out_offset == duration / 2.
	let transition = video.children()[1]
		.as_transition()
		.expect("spine child 1 is a Transition");
	assert_eq!(transition.name(), "Cross Dissolve");
	assert_time(transition.in_offset(), 0.6, 30.0);
	assert_time(transition.out_offset(), 0.6, 30.0);

	// Disabled second clip referencing the same asset.
	let clip_b = video.children()[2]
		.as_clip()
		.expect("spine child 2 is a Clip");
	assert_eq!(clip_b.name(), "Clip B");
	assert!(!clip_b.enabled());
	let range = clip_b.source_range().expect("Clip B has a source range");
	assert_time(range.start_time(), 0.0, 30.0);
	assert_time(range.duration(), 6.0, 30.0);

	// Gap with a name.
	let gap = video.children()[3]
		.as_gap()
		.expect("spine child 3 is a Gap");
	assert_eq!(gap.name(), "Tail");
	let grange = gap.source_range().expect("gap source range");
	assert_eq!(grange.start_time().to_rational(), Rational::new(31, 50));
	assert_eq!(grange.duration().to_rational(), Rational::new(1, 50));

	// Title is not mapped; its timing becomes an unnamed gap.
	let title_gap = video.children()[4]
		.as_gap()
		.expect("spine child 4 is a Gap (from title)");
	assert_eq!(title_gap.name(), "");
	let trange = title_gap.source_range().expect("title gap source range");
	assert_eq!(trange.start_time().to_rational(), Rational::new(16, 25));
	assert_eq!(trange.duration().to_rational(), Rational::new(1, 10));

	// Audio track with NTSC material: 3 frames at 30000/1001.
	assert_eq!(audio.children().len(), 1);
	let ambience = audio.children()[0]
		.as_clip()
		.expect("audio child 0 is a Clip");
	assert_eq!(ambience.name(), "Ambience");
	let range = ambience.source_range().expect("Ambience source range");
	assert_time(range.start_time(), 0.0, NTSC_RATE);
	assert_time(range.duration(), 3.0, NTSC_RATE);
	let external = match ambience.media_reference().unwrap() {
		MediaReference::ExternalReference(e) => e,
		other => panic!("expected ExternalReference, got {}", other.schema_name()),
	};
	assert_eq!(external.target_url(), "file:///tmp/Ambience.wav");
	let available = external.available_range().unwrap();
	assert_time(available.duration(), 3.0, NTSC_RATE);
}

#[test]
fn model_round_trips_through_fcpxml() {
	let original = parse_synthetic();
	let xml = oak_otio::to_fcpxml_string(std::slice::from_ref(&original)).expect("export fcpxml");
	let reparsed = oak_otio::from_fcpxml_string(&xml).expect("reparse exported fcpxml");
	assert_eq!(reparsed.len(), 1);
	assert_eq!(reparsed[0], original, "model survives fcpxml round trip");
}

#[test]
fn exported_document_structure() {
	let timeline = parse_synthetic();
	let xml = oak_otio::to_fcpxml_string(&[timeline]).expect("export fcpxml");

	// Document scaffolding.
	assert!(
		xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
		"{xml}"
	);
	assert!(xml.contains("<!DOCTYPE fcpxml>"), "{xml}");
	assert!(xml.contains("<fcpxml version=\"1.10\">"), "{xml}");
	assert!(xml.contains("</library>"), "{xml}");
	assert!(xml.contains("</fcpxml>"), "{xml}");

	// Resources: two formats (30 fps + NTSC), two assets (video + audio).
	assert!(xml.contains("<format id=\"r1\" name=\"FFVideoFormat1920x1080p30\" frameDuration=\"1/30s\" width=\"1920\" height=\"1080\"/>"), "{xml}");
	assert!(xml.contains("<format id=\"r2\" name=\"FFVideoFormat1920x1080p29_97\" frameDuration=\"1001/30000s\" width=\"1920\" height=\"1080\"/>"), "{xml}");
	assert!(xml.contains("<asset id=\"r3\" name=\"Clip A.mov\" src=\"file:///tmp/Clip A.mov\" format=\"r1\" duration=\"4s\" hasVideo=\"1\" hasAudio=\"0\"/>"), "{xml}");
	assert!(xml.contains("<asset id=\"r4\" name=\"Ambience.wav\" src=\"file:///tmp/Ambience.wav\" format=\"r2\" duration=\"1001/10000s\" hasVideo=\"0\" hasAudio=\"1\"/>"), "{xml}");

	// Sequence with preserved interchange hints.
	assert!(xml.contains("<sequence format=\"r1\" tcStart=\"6/5s\" tcFormat=\"NDF\" duration=\"37/50s\" audioLayout=\"stereo\" audioRate=\"48000\" name=\"My Project\">"), "{xml}");

	// Spine blocks with exact reduced-rational time values.
	assert!(xml.contains("<spine>"), "{xml}");
	assert!(xml.contains("<asset-clip name=\"Clip A\" ref=\"r3\" offset=\"0s\" duration=\"2/5s\" start=\"4/5s\"/>"), "{xml}");
	assert!(
		xml.contains("<transition name=\"Cross Dissolve\" offset=\"19/50s\" duration=\"1/25s\"/>"),
		"{xml}"
	);
	assert!(xml.contains("<asset-clip name=\"Clip B\" ref=\"r3\" offset=\"21/50s\" duration=\"1/5s\" start=\"0s\" enabled=\"0\"/>"), "{xml}");
	assert!(
		xml.contains("<gap name=\"Tail\" offset=\"31/50s\" duration=\"1/50s\"/>"),
		"{xml}"
	);
	assert!(
		xml.contains("<gap name=\"\" offset=\"16/25s\" duration=\"1/10s\"/>"),
		"{xml}"
	);
	assert!(xml.contains("</spine>"), "{xml}");

	// Secondary audio track with an NTSC clip.
	assert!(xml.contains("<audio>"), "{xml}");
	assert!(xml.contains("<asset-clip name=\"Ambience\" ref=\"r4\" offset=\"0s\" duration=\"1001/10000s\" start=\"0s\" format=\"r2\"/>"), "{xml}");
	assert!(xml.contains("</audio>"), "{xml}");
}

#[test]
fn ntsc_frame_accuracy() {
	// A built model with NTSC media round-trips frame-exactly.
	let mut timeline = Timeline::new("NTSC");
	let mut video = Track::new("Video");
	let mut clip = Clip::new("ntsc");
	clip.set_source_range(oak_otio::TimeRange::new(
		RationalTime::new(0.0, NTSC_RATE),
		RationalTime::new(3.0, NTSC_RATE),
	));
	clip.set_media_reference(MediaReference::ExternalReference(ExternalReference::new(
		"file:///tmp/ntsc.mov",
		Some(oak_otio::TimeRange::new(
			RationalTime::new(0.0, NTSC_RATE),
			RationalTime::new(1000.0, NTSC_RATE),
		)),
	)));
	video.append_child(Composable::Clip(clip));
	timeline.tracks_mut().append_child(Composable::Track(video));

	let xml = oak_otio::to_fcpxml_string(&[timeline]).expect("export");
	let reparsed = oak_otio::from_fcpxml_string(&xml).expect("reparse");
	let clip = reparsed[0].tracks().children()[0]
		.as_track()
		.unwrap()
		.children()[0]
		.as_clip()
		.unwrap();
	let range = clip.source_range().unwrap();
	// 3 frames at 30000/1001 stay 3 frames through seconds.
	assert_time(range.duration(), 3.0, NTSC_RATE);
	// to_rational is the exact seconds value (3 frames = 1001/10000 s).
	assert_eq!(range.duration().to_rational(), Rational::new(1001, 10000));
	let available = clip
		.media_reference()
		.unwrap()
		.as_external_reference()
		.unwrap()
		.available_range()
		.unwrap();
	assert_time(available.duration(), 1000.0, NTSC_RATE);
}

#[test]
fn error_corrupt_xml() {
	// Mismatched closing tag. quick-xml reports it as an ill-formed
	// document (Xml); the crate also classifies structural errors as
	// Malformed — either is a hard error.
	let bad = "<fcpxml version=\"1.10\"><resources></fcpxml>";
	assert!(matches!(
		oak_otio::from_fcpxml_string(bad),
		Err(oak_otio::FcpxmlError::Xml(_)) | Err(oak_otio::FcpxmlError::Malformed(_))
	));

	// Unclosed root element.
	let bad = "<fcpxml version=\"1.10\">";
	assert!(matches!(
		oak_otio::from_fcpxml_string(bad),
		Err(oak_otio::FcpxmlError::Malformed(_))
	));

	// Truly broken markup.
	let bad = "<fcpxml version=\"1.10\"><resources><format";
	assert!(matches!(
		oak_otio::from_fcpxml_string(bad),
		Err(oak_otio::FcpxmlError::Xml(_))
	));

	// Wrong root element.
	let bad = "<root/>";
	assert!(matches!(
		oak_otio::from_fcpxml_string(bad),
		Err(oak_otio::FcpxmlError::Malformed(_))
	));
}

#[test]
fn error_missing_resources() {
	// A sequence referencing a format that does not exist is an error.
	let doc = r#"<fcpxml version="1.10">
	<resources><format id="f1" frameDuration="100/3000s"/></resources>
	<library><event name="e"><project name="p">
		<sequence format="nope">
			<spine/>
		</sequence>
	</project></event></library>
</fcpxml>"#;
	match oak_otio::from_fcpxml_string(doc) {
		Err(oak_otio::FcpxmlError::Malformed(msg)) => {
			assert!(msg.contains("nope"), "{msg}");
		}
		other => panic!("expected Malformed error, got {other:?}"),
	}

	// A sequence without any format is an error too.
	let doc = r#"<fcpxml version="1.10">
	<library><event name="e"><project name="p">
		<sequence><spine/></sequence>
	</project></event></library>
</fcpxml>"#;
	assert!(matches!(
		oak_otio::from_fcpxml_string(doc),
		Err(oak_otio::FcpxmlError::Malformed(_))
	));
}

#[test]
fn error_unknown_version() {
	// Unsupported version.
	let doc = "<fcpxml version=\"2.0\"><resources/></fcpxml>";
	assert!(matches!(
		oak_otio::from_fcpxml_string(doc),
		Err(oak_otio::FcpxmlError::UnsupportedVersion(_))
	));

	// Missing version.
	let doc = "<fcpxml><resources/></fcpxml>";
	assert!(matches!(
		oak_otio::from_fcpxml_string(doc),
		Err(oak_otio::FcpxmlError::UnsupportedVersion(_))
	));

	// Garbage version.
	let doc = "<fcpxml version=\"bogus\"><resources/></fcpxml>";
	assert!(matches!(
		oak_otio::from_fcpxml_string(doc),
		Err(oak_otio::FcpxmlError::UnsupportedVersion(_))
	));
}

#[test]
fn lenient_unknown_elements() {
	let doc = r#"<fcpxml version="1.10">
	<resources>
		<format id="f1" frameDuration="100/3000s" width="1920" height="1080"/>
		<asset id="a1" src="file:///x.mov" format="f1" duration="10s"/>
		<weird-resource foo="bar"/>
	</resources>
	<library>
		<event name="E">
			<project name="P">
				<sequence format="f1" tcStart="0s" tcFormat="NDF">
					<spine>
						<sync-clip name="MC" offset="0s" duration="300/3000s"/>
						<asset-clip name="X" ref="a1" offset="300/3000s" duration="300/3000s" start="0s" future-attr="42"/>
						<bogus-element/>
						<gap name="G" offset="600/3000s" duration="300/3000s"/>
						<title name="T" offset="900/3000s" duration="300/3000s"/>
						<asset-clip name="NoMedia" offset="1200/3000s" duration="300/3000s" start="0s"/>
					</spine>
				</sequence>
			</project>
		</event>
	</library>
</fcpxml>"#;
	let timelines = oak_otio::from_fcpxml_string(doc).expect("lenient document parses");
	assert_eq!(timelines.len(), 1);
	let timeline = &timelines[0];
	assert_eq!(timeline.name(), "P");

	let track = timeline.tracks().children()[0].as_track().unwrap();
	// sync-clip, X, G, title, NoMedia (the bogus element is skipped).
	assert_eq!(track.children().len(), 5);

	let mc = track.children()[0].as_gap().expect("sync-clip -> gap");
	assert_eq!(
		mc.source_range().unwrap().duration().to_rational(),
		Rational::new(1, 10)
	);

	let x = track.children()[1].as_clip().expect("asset-clip");
	assert_eq!(x.name(), "X");
	match x.media_reference().unwrap() {
		MediaReference::ExternalReference(e) => assert_eq!(e.target_url(), "file:///x.mov"),
		other => panic!("expected ExternalReference, got {}", other.schema_name()),
	}

	let gap = track.children()[2].as_gap().expect("named gap");
	assert_eq!(gap.name(), "G");

	let title = track.children()[3].as_gap().expect("title -> gap");
	assert_eq!(
		title.source_range().unwrap().duration().to_rational(),
		Rational::new(1, 10)
	);

	let no_media = track.children()[4].as_clip().expect("clip without ref");
	assert!(matches!(
		no_media.media_reference().unwrap(),
		MediaReference::MissingReference(_)
	));
}

#[test]
fn multiple_timelines_export_and_import() {
	let mut a = Timeline::new("Sequence One");
	let mut track_a = Track::new("Video");
	let mut clip = Clip::new("c1");
	clip.set_source_range(oak_otio::TimeRange::new(
		RationalTime::new(0.0, 24.0),
		RationalTime::new(48.0, 24.0),
	));
	clip.set_media_reference(MediaReference::ExternalReference(ExternalReference::new(
		"file:///tmp/one.mov",
		None,
	)));
	track_a.append_child(Composable::Clip(clip));
	a.tracks_mut().append_child(Composable::Track(track_a));

	let mut b = Timeline::new("Sequence Two");
	let mut track_b = Track::new("Audio");
	let mut clip2 = Clip::new("c2");
	clip2.set_source_range(oak_otio::TimeRange::new(
		RationalTime::new(0.0, 48000.0),
		RationalTime::new(96000.0, 48000.0),
	));
	clip2.set_media_reference(MediaReference::ExternalReference(ExternalReference::new(
		"file:///tmp/two.wav",
		None,
	)));
	track_b.append_child(Composable::Clip(clip2));
	b.tracks_mut().append_child(Composable::Track(track_b));

	let xml = oak_otio::to_fcpxml_string(&[a.clone(), b.clone()]).expect("export two timelines");
	assert!(xml.contains("<event"), "{xml}");
	assert!(xml.contains("name=\"Sequence One\""), "{xml}");
	assert!(xml.contains("name=\"Sequence Two\""), "{xml}");

	let reparsed = oak_otio::from_fcpxml_string(&xml).expect("reparse");
	assert_eq!(reparsed.len(), 2);
	assert_eq!(reparsed[0].name(), "Sequence One");
	assert_eq!(reparsed[1].name(), "Sequence Two");

	// A clip without an available_range gains one on export (the asset
	// duration defaults to the clip length), so compare the round-trip
	// parts the model carries.
	let clip_rt = reparsed[0].tracks().children()[0]
		.as_track()
		.unwrap()
		.children()[0]
		.as_clip()
		.unwrap();
	assert_eq!(clip_rt.name(), "c1");
	let range = clip_rt.source_range().unwrap();
	assert_time(range.duration(), 48.0, 24.0);
	let available = clip_rt
		.media_reference()
		.unwrap()
		.as_external_reference()
		.unwrap()
		.available_range()
		.unwrap();
	// The synthesized asset duration equals the clip length (48 frames
	// at 24 fps = 2 s).
	assert_time(available.duration(), 48.0, 24.0);
	assert_eq!(available.duration().to_rational(), Rational::new(2, 1));
}

#[test]
fn file_round_trip() {
	let path = std::env::temp_dir().join(format!("oakotio_fcpxml_{}.fcpxml", std::process::id()));
	oak_otio::to_fcpxml_file(&[parse_synthetic()], &path).expect("write fcpxml file");
	let timelines = oak_otio::from_fcpxml_file(&path).expect("read fcpxml file");
	assert_eq!(timelines.len(), 1);
	assert_eq!(timelines[0].name(), "My Project");
	std::fs::remove_file(&path).ok();
}

#[test]
fn empty_timeline_list() {
	let xml = oak_otio::to_fcpxml_string(&[]).expect("export empty list");
	assert!(xml.contains("<fcpxml version=\"1.10\">"), "{xml}");
	let timelines = oak_otio::from_fcpxml_string(&xml).expect("reparse");
	assert!(timelines.is_empty());
}

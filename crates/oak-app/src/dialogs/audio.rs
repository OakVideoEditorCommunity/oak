use super::*;

/// Audio properties editor: per-clip gain/pan/fades plus Volume/Pan effect
/// parameters. The host applies through the SDK's undoable audio commands.
pub struct AudioPropertiesContent {
	gain: Entity<SpinBox>,
	pan: Entity<SpinBox>,
	fade_in: Entity<SpinBox>,
	fade_out: Entity<SpinBox>,
	volume_enabled: Entity<CheckBox>,
	volume: Entity<SpinBox>,
	pan_effect_enabled: Entity<CheckBox>,
	pan_effect: Entity<SpinBox>,
	eq_enabled: Entity<CheckBox>,
	eq_low: Entity<SpinBox>,
	eq_mid: Entity<SpinBox>,
	eq_high: Entity<SpinBox>,
	comp_enabled: Entity<CheckBox>,
	comp_threshold: Entity<SpinBox>,
	comp_ratio: Entity<SpinBox>,
	comp_attack: Entity<SpinBox>,
	comp_release: Entity<SpinBox>,
	comp_makeup: Entity<SpinBox>,
	limiter_enabled: Entity<CheckBox>,
	limiter_ceiling: Entity<SpinBox>,
	limiter_release: Entity<SpinBox>,
	gate_enabled: Entity<CheckBox>,
	gate_threshold: Entity<SpinBox>,
	gate_attack: Entity<SpinBox>,
	gate_release: Entity<SpinBox>,
	gate_hold: Entity<SpinBox>,
	duck_enabled: Entity<CheckBox>,
	duck_threshold: Entity<SpinBox>,
	duck_amount: Entity<SpinBox>,
	duck_attack: Entity<SpinBox>,
	duck_release: Entity<SpinBox>,
	bus_limiter_enabled: Entity<CheckBox>,
	bus_limiter_ceiling: Entity<SpinBox>,
	bus_limiter_release: Entity<SpinBox>,
	loudness: Option<quercus_engine_rs::editor::LoudnessMeasurement>,
	initial_effects: Vec<quercus_engine_rs::editor::ClipAudioEffect>,
	error: Option<String>,
}

impl AudioPropertiesContent {
	/// Builds the audio properties editor seeded from `audio` and `bus`.
	///
	/// `audio` is the clip's current audio state: `gain` (`0.0..=4.0`, `1.0` =
	/// unity), `pan` (`-1.0..=1.0`), `fade_in`/`fade_out` (seconds, shown in a
	/// `0.0..=10.0` spin box) seed the mix row, and `audio.effects` drives the
	/// effect sections — a section is checked exactly when an effect with that
	/// type id is present, and each parameter falls back to the documented
	/// slider default when the effect or parameter is missing (Volume `1.0`,
	/// Pan `0.0`, EQ gains `0.0`, compressor threshold `-24.0`/ratio `4.0`/
	/// attack `10.0`/release `100.0`/makeup `0.0`, limiter ceiling `-1.0`/
	/// release `50.0`, gate threshold `-40.0`/attack `5.0`/release `100.0`/
	/// hold `20.0`, ducking threshold `-30.0`/amount `-12.0`/attack `50.0`/
	/// release `300.0`).
	///
	/// `bus` is the master-bus effect chain; only its limiter entry is read
	/// (ceiling `-1.0`, release `50.0` as fallbacks). `window` is needed for the
	/// input widgets and `cx` for the new entities, and every checkbox is
	/// subscribed with [`subscribe_checkbox`]. The incoming effect list is
	/// cloned into [`Self::initial_effects`] so the host can diff it against the
	/// edited state, and the loudness row and error start empty.
	pub fn new(
		audio: &quercus_engine_rs::editor::ClipAudioState,
		bus: &[quercus_engine_rs::editor::AudioBusEffect],
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let volume_effect = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.volume");
		let pan_effect_entry = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.pan");
		let gain = cx.new(|cx| {
			SpinBox::new(
				91,
				SliderModel::new(ValueKind::Float, 0.0, 4.0, 0.05, audio.gain),
				window,
				cx,
			)
		});
		let pan = cx.new(|cx| {
			SpinBox::new(
				92,
				SliderModel::new(ValueKind::Float, -1.0, 1.0, 0.05, audio.pan),
				window,
				cx,
			)
		});
		let fade_in = cx.new(|cx| {
			SpinBox::new(
				93,
				SliderModel::new(ValueKind::Float, 0.0, 10.0, 0.1, audio.fade_in.to_f64()),
				window,
				cx,
			)
		});
		let fade_out = cx.new(|cx| {
			SpinBox::new(
				94,
				SliderModel::new(ValueKind::Float, 0.0, 10.0, 0.1, audio.fade_out.to_f64()),
				window,
				cx,
			)
		});
		let volume_enabled = cx.new(|cx| {
			CheckBox::new(
				95,
				if volume_effect.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.volume_effect"))
		});
		subscribe_checkbox(&volume_enabled, cx);
		let volume = cx.new(|cx| {
			SpinBox::new(
				96,
				SliderModel::new(
					ValueKind::Float,
					0.0,
					4.0,
					0.05,
					volume_effect
						.and_then(|effect| effect.volume)
						.unwrap_or(1.0),
				),
				window,
				cx,
			)
		});
		let pan_effect_enabled = cx.new(|cx| {
			CheckBox::new(
				97,
				if pan_effect_entry.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.pan_effect"))
		});
		subscribe_checkbox(&pan_effect_enabled, cx);
		let pan_effect = cx.new(|cx| {
			SpinBox::new(
				98,
				SliderModel::new(
					ValueKind::Float,
					-1.0,
					1.0,
					0.05,
					pan_effect_entry
						.and_then(|effect| effect.pan)
						.unwrap_or(0.0),
				),
				window,
				cx,
			)
		});
		let eq = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.eq");
		let eq_param = |input: &str, default: f64| {
			eq.and_then(|effect| {
				effect
					.params
					.iter()
					.find(|(id, _)| id == input)
					.map(|(_, value)| *value)
			})
			.unwrap_or(default)
		};
		let comp = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.compressor");
		let comp_param = |input: &str, default: f64| {
			comp.and_then(|effect| {
				effect
					.params
					.iter()
					.find(|(id, _)| id == input)
					.map(|(_, value)| *value)
			})
			.unwrap_or(default)
		};
		let eq_enabled = cx.new(|cx| {
			CheckBox::new(
				100,
				if eq.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.eq_effect"))
		});
		subscribe_checkbox(&eq_enabled, cx);
		let eq_low = cx.new(|cx| {
			SpinBox::new(
				101,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					24.0,
					1.0,
					eq_param("low_gain_db", 0.0),
				),
				window,
				cx,
			)
		});
		let eq_mid = cx.new(|cx| {
			SpinBox::new(
				102,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					24.0,
					1.0,
					eq_param("mid_gain_db", 0.0),
				),
				window,
				cx,
			)
		});
		let eq_high = cx.new(|cx| {
			SpinBox::new(
				103,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					24.0,
					1.0,
					eq_param("high_gain_db", 0.0),
				),
				window,
				cx,
			)
		});
		let comp_enabled = cx.new(|cx| {
			CheckBox::new(
				104,
				if comp.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.comp_effect"))
		});
		subscribe_checkbox(&comp_enabled, cx);
		let comp_threshold = cx.new(|cx| {
			SpinBox::new(
				105,
				SliderModel::new(
					ValueKind::Float,
					-60.0,
					0.0,
					1.0,
					comp_param("threshold_db", -24.0),
				),
				window,
				cx,
			)
		});
		let comp_ratio = cx.new(|cx| {
			SpinBox::new(
				106,
				SliderModel::new(ValueKind::Float, 1.0, 20.0, 0.1, comp_param("ratio", 4.0)),
				window,
				cx,
			)
		});
		let comp_attack = cx.new(|cx| {
			SpinBox::new(
				107,
				SliderModel::new(
					ValueKind::Float,
					0.1,
					200.0,
					0.1,
					comp_param("attack_ms", 10.0),
				),
				window,
				cx,
			)
		});
		let comp_release = cx.new(|cx| {
			SpinBox::new(
				108,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					2000.0,
					1.0,
					comp_param("release_ms", 100.0),
				),
				window,
				cx,
			)
		});
		let comp_makeup = cx.new(|cx| {
			SpinBox::new(
				109,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					24.0,
					1.0,
					comp_param("makeup_db", 0.0),
				),
				window,
				cx,
			)
		});
		let limiter = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.limiter");
		let limiter_param = |input: &str, default: f64| {
			limiter
				.and_then(|effect| {
					effect
						.params
						.iter()
						.find(|(id, _)| id == input)
						.map(|(_, v)| *v)
				})
				.unwrap_or(default)
		};
		let limiter_enabled = cx.new(|cx| {
			CheckBox::new(
				110,
				if limiter.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.limiter_effect"))
		});
		subscribe_checkbox(&limiter_enabled, cx);
		let limiter_ceiling = cx.new(|cx| {
			SpinBox::new(
				111,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					0.0,
					0.1,
					limiter_param("ceiling_db", -1.0),
				),
				window,
				cx,
			)
		});
		let limiter_release = cx.new(|cx| {
			SpinBox::new(
				112,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					1000.0,
					1.0,
					limiter_param("release_ms", 50.0),
				),
				window,
				cx,
			)
		});
		let gate = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.gate");
		let gate_param = |input: &str, default: f64| {
			gate.and_then(|effect| {
				effect
					.params
					.iter()
					.find(|(id, _)| id == input)
					.map(|(_, v)| *v)
			})
			.unwrap_or(default)
		};
		let gate_enabled = cx.new(|cx| {
			CheckBox::new(
				113,
				if gate.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.gate_effect"))
		});
		subscribe_checkbox(&gate_enabled, cx);
		let gate_threshold = cx.new(|cx| {
			SpinBox::new(
				114,
				SliderModel::new(
					ValueKind::Float,
					-80.0,
					0.0,
					1.0,
					gate_param("threshold_db", -40.0),
				),
				window,
				cx,
			)
		});
		let gate_attack = cx.new(|cx| {
			SpinBox::new(
				115,
				SliderModel::new(
					ValueKind::Float,
					0.1,
					200.0,
					0.1,
					gate_param("attack_ms", 5.0),
				),
				window,
				cx,
			)
		});
		let gate_release = cx.new(|cx| {
			SpinBox::new(
				116,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					2000.0,
					1.0,
					gate_param("release_ms", 100.0),
				),
				window,
				cx,
			)
		});
		let gate_hold = cx.new(|cx| {
			SpinBox::new(
				117,
				SliderModel::new(
					ValueKind::Float,
					0.0,
					500.0,
					1.0,
					gate_param("hold_ms", 20.0),
				),
				window,
				cx,
			)
		});
		let duck = audio
			.effects
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.ducking");
		let duck_param = |input: &str, default: f64| {
			duck.and_then(|effect| {
				effect
					.params
					.iter()
					.find(|(id, _)| id == input)
					.map(|(_, v)| *v)
			})
			.unwrap_or(default)
		};
		let duck_enabled = cx.new(|cx| {
			CheckBox::new(
				118,
				if duck.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.duck_effect"))
		});
		subscribe_checkbox(&duck_enabled, cx);
		let duck_threshold = cx.new(|cx| {
			SpinBox::new(
				119,
				SliderModel::new(
					ValueKind::Float,
					-80.0,
					0.0,
					1.0,
					duck_param("threshold_db", -30.0),
				),
				window,
				cx,
			)
		});
		let duck_amount = cx.new(|cx| {
			SpinBox::new(
				120,
				SliderModel::new(
					ValueKind::Float,
					-40.0,
					0.0,
					1.0,
					duck_param("amount_db", -12.0),
				),
				window,
				cx,
			)
		});
		let duck_attack = cx.new(|cx| {
			SpinBox::new(
				121,
				SliderModel::new(
					ValueKind::Float,
					0.1,
					500.0,
					0.1,
					duck_param("attack_ms", 50.0),
				),
				window,
				cx,
			)
		});
		let duck_release = cx.new(|cx| {
			SpinBox::new(
				122,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					3000.0,
					1.0,
					duck_param("release_ms", 300.0),
				),
				window,
				cx,
			)
		});
		let bus_limiter = bus
			.iter()
			.find(|effect| effect.type_id == "org.olivevideoeditor.Olive.limiter");
		let bus_param = |input: &str, default: f64| {
			bus_limiter
				.and_then(|effect| {
					effect
						.params
						.iter()
						.find(|(id, _)| id == input)
						.map(|(_, v)| *v)
				})
				.unwrap_or(default)
		};
		let bus_limiter_enabled = cx.new(|cx| {
			CheckBox::new(
				130,
				if bus_limiter.is_some() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("audio.props.bus_limiter"))
		});
		subscribe_checkbox(&bus_limiter_enabled, cx);
		let bus_limiter_ceiling = cx.new(|cx| {
			SpinBox::new(
				131,
				SliderModel::new(
					ValueKind::Float,
					-24.0,
					0.0,
					0.1,
					bus_param("ceiling_db", -1.0),
				),
				window,
				cx,
			)
		});
		let bus_limiter_release = cx.new(|cx| {
			SpinBox::new(
				132,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					1000.0,
					1.0,
					bus_param("release_ms", 50.0),
				),
				window,
				cx,
			)
		});
		Self {
			gain,
			pan,
			fade_in,
			fade_out,
			volume_enabled,
			volume,
			pan_effect_enabled,
			pan_effect,
			eq_enabled,
			eq_low,
			eq_mid,
			eq_high,
			comp_enabled,
			comp_threshold,
			comp_ratio,
			comp_attack,
			comp_release,
			comp_makeup,
			limiter_enabled,
			limiter_ceiling,
			limiter_release,
			gate_enabled,
			gate_threshold,
			gate_attack,
			gate_release,
			gate_hold,
			duck_enabled,
			duck_threshold,
			duck_amount,
			duck_attack,
			duck_release,
			bus_limiter_enabled,
			bus_limiter_ceiling,
			bus_limiter_release,
			loudness: None,
			initial_effects: audio.effects.clone(),
			error: None,
		}
	}

	/// Returns the per-clip mix controls as
	/// `(gain, pan, fade_in_seconds, fade_out_seconds)`.
	///
	/// `gain` is linear (`0.0..=4.0`, `1.0` = unity), `pan` runs `-1.0` (hard
	/// left) to `1.0` (hard right), and the fade lengths are in seconds
	/// (`0.0..=10.0`). The host multiplies the two fades by 1000 before passing
	/// them to the engine, which expects milliseconds. `cx` is only used to read
	/// the widgets.
	pub fn mix(&self, cx: &App) -> (f64, f64, f64, f64) {
		(
			self.gain.read(cx).value().to_f64(),
			self.pan.read(cx).value().to_f64(),
			self.fade_in.read(cx).value().to_f64(),
			self.fade_out.read(cx).value().to_f64(),
		)
	}

	/// Collects the enabled clip audio effects for the commit.
	///
	/// Returns one `(effect_type_id, params)` pair per checked effect section,
	/// in the dialog's fixed chain order: Volume, Pan, EQ, Compressor, Limiter,
	/// Gate, Ducking. Unchecked sections are omitted entirely. `params` holds
	/// `(input_id, value)` pairs in that effect's own order — Volume sends
	/// `volume_in`, Pan `panning_in`, EQ `low_gain_db`/`mid_gain_db`/
	/// `high_gain_db`, the compressor threshold/ratio/attack/release/makeup, the
	/// limiter ceiling/release, the gate threshold/attack/release/hold, and
	/// ducking threshold/amount/attack/release. Values are raw slider values in
	/// their natural units (dB for gains and thresholds, ms for times, `1.0` =
	/// unity volume). The host diffs the result against
	/// [`Self::initial_effects`] to derive updates, additions and removals; `cx`
	/// is only used to read the widgets.
	pub fn effects(&self, cx: &App) -> Vec<(String, Vec<(String, f64)>)> {
		let mut effects = Vec::new();
		if self.volume_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.volume".to_string(),
				vec![(
					"volume_in".to_string(),
					self.volume.read(cx).value().to_f64(),
				)],
			));
		}
		if self.pan_effect_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.pan".to_string(),
				vec![(
					"panning_in".to_string(),
					self.pan_effect.read(cx).value().to_f64(),
				)],
			));
		}
		if self.eq_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.eq".to_string(),
				vec![
					(
						"low_gain_db".to_string(),
						self.eq_low.read(cx).value().to_f64(),
					),
					(
						"mid_gain_db".to_string(),
						self.eq_mid.read(cx).value().to_f64(),
					),
					(
						"high_gain_db".to_string(),
						self.eq_high.read(cx).value().to_f64(),
					),
				],
			));
		}
		if self.comp_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.compressor".to_string(),
				vec![
					(
						"threshold_db".to_string(),
						self.comp_threshold.read(cx).value().to_f64(),
					),
					(
						"ratio".to_string(),
						self.comp_ratio.read(cx).value().to_f64(),
					),
					(
						"attack_ms".to_string(),
						self.comp_attack.read(cx).value().to_f64(),
					),
					(
						"release_ms".to_string(),
						self.comp_release.read(cx).value().to_f64(),
					),
					(
						"makeup_db".to_string(),
						self.comp_makeup.read(cx).value().to_f64(),
					),
				],
			));
		}
		if self.limiter_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.limiter".to_string(),
				vec![
					(
						"ceiling_db".to_string(),
						self.limiter_ceiling.read(cx).value().to_f64(),
					),
					(
						"release_ms".to_string(),
						self.limiter_release.read(cx).value().to_f64(),
					),
				],
			));
		}
		if self.gate_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.gate".to_string(),
				vec![
					(
						"threshold_db".to_string(),
						self.gate_threshold.read(cx).value().to_f64(),
					),
					(
						"attack_ms".to_string(),
						self.gate_attack.read(cx).value().to_f64(),
					),
					(
						"release_ms".to_string(),
						self.gate_release.read(cx).value().to_f64(),
					),
					(
						"hold_ms".to_string(),
						self.gate_hold.read(cx).value().to_f64(),
					),
				],
			));
		}
		if self.duck_enabled.read(cx).state() == CheckState::Checked {
			effects.push((
				"org.olivevideoeditor.Olive.ducking".to_string(),
				vec![
					(
						"threshold_db".to_string(),
						self.duck_threshold.read(cx).value().to_f64(),
					),
					(
						"amount_db".to_string(),
						self.duck_amount.read(cx).value().to_f64(),
					),
					(
						"attack_ms".to_string(),
						self.duck_attack.read(cx).value().to_f64(),
					),
					(
						"release_ms".to_string(),
						self.duck_release.read(cx).value().to_f64(),
					),
				],
			));
		}
		effects
	}

	/// Returns the master (bus) limiter controls as
	/// `(enabled, ceiling_db, release_ms)`.
	///
	/// `enabled` is the bus-limiter checkbox state, `ceiling_db` the ceiling in
	/// dBFS (widget range `-24.0..=0.0`, default `-1.0`) and `release_ms` the
	/// release time in milliseconds (range `1.0..=1000.0`, default `50.0`).
	/// The host forwards the triple to the engine's `set_audio_bus_limiter` when
	/// committing the dialog; `cx` is only used to read the widgets.
	pub fn bus_limiter(&self, cx: &App) -> (bool, f64, f64) {
		(
			self.bus_limiter_enabled.read(cx).state() == CheckState::Checked,
			self.bus_limiter_ceiling.read(cx).value().to_f64(),
			self.bus_limiter_release.read(cx).value().to_f64(),
		)
	}

	/// Shows an integrated-LUFS / peak measurement under the audio form.
	///
	/// `loudness` is the measurement to display (`None` hides the row); `cx` is
	/// notified so the dialog repaints. The text is formatted as
	/// "N.N LUFS / N.N dBFS". This is display-only: the value is not read by
	/// [`Self::mix`], [`Self::effects`] or [`Self::bus_limiter`], so it never
	/// feeds back into the commit.
	pub fn set_loudness(
		&mut self,
		loudness: Option<quercus_engine_rs::editor::LoudnessMeasurement>,
		cx: &mut Context<Self>,
	) {
		self.loudness = loudness;
		cx.notify();
	}

	/// The effect chain as it was when the dialog opened.
	pub fn initial_effects(&self) -> &[quercus_engine_rs::editor::ClipAudioEffect] {
		&self.initial_effects
	}

	/// Sets or clears the validation error shown under the audio form.
	///
	/// `error` is rendered in red below the fields while it is `Some`; passing
	/// `None` removes the row. `cx` is notified so the dialog repaints. The host
	/// calls this with the engine's message when the commit is rejected, and the
	/// dialog stays open so the user can correct the values.
	pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
		self.error = error;
		cx.notify();
	}
}

impl Render for AudioPropertiesContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("audio.props.gain").into(),
				self.gain.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.pan").into(),
				self.pan.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.fade_in").into(),
				self.fade_in.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.fade_out").into(),
				self.fade_out.clone(),
			))
			.child(self.volume_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.volume").into(),
				self.volume.clone(),
			))
			.child(self.pan_effect_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.pan_value").into(),
				self.pan_effect.clone(),
			))
			.child(self.eq_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.eq_low").into(),
				self.eq_low.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.eq_mid").into(),
				self.eq_mid.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.eq_high").into(),
				self.eq_high.clone(),
			))
			.child(self.comp_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.comp_threshold").into(),
				self.comp_threshold.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.comp_ratio").into(),
				self.comp_ratio.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.comp_attack").into(),
				self.comp_attack.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.comp_release").into(),
				self.comp_release.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.comp_makeup").into(),
				self.comp_makeup.clone(),
			))
			.child(self.limiter_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.limiter_ceiling").into(),
				self.limiter_ceiling.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.limiter_release").into(),
				self.limiter_release.clone(),
			))
			.child(self.gate_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.gate_threshold").into(),
				self.gate_threshold.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.gate_attack").into(),
				self.gate_attack.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.gate_release").into(),
				self.gate_release.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.gate_hold").into(),
				self.gate_hold.clone(),
			))
			.child(self.duck_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.duck_threshold").into(),
				self.duck_threshold.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.duck_amount").into(),
				self.duck_amount.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.duck_attack").into(),
				self.duck_attack.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.duck_release").into(),
				self.duck_release.clone(),
			))
			.child(
				gpui_kit::div()
					.h_px()
					.border_t_1()
					.border_color(colors.border),
			)
			.child(self.bus_limiter_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("audio.props.bus_limiter_ceiling").into(),
				self.bus_limiter_ceiling.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("audio.props.bus_limiter_release").into(),
				self.bus_limiter_release.clone(),
			))
			.child(if let Some(l) = self.loudness {
				gpui_kit::div().text_xs().child(format!(
					"{}: {:.1} LUFS / {:.1} dBFS",
					i18n::tr("audio.props.loudness"),
					l.integrated_lufs,
					l.peak_dbfs
				))
			} else {
				gpui_kit::div()
			})
			.child(if let Some(error) = &self.error {
				div()
					.debug_selector(|| "audio-props-error".into())
					.text_color(gpui_kit::rgb(0xe5484d))
					.text_xs()
					.child(error.clone())
			} else {
				div()
			})
	}
}

/// Creates a single-line text field already holding `value`.
///
/// `value` is moved into a new [`TextValue`] that wraps a fresh [`InputState`]
/// built against `window`; `cx` is the caller's context and is used both to
/// create the nested input entity and to seed the text. `T` is only the caller's
/// view type, it is not stored anywhere. The returned entity is readable
/// immediately, so callers can inspect the seeded text without another update;
/// an empty `value` yields an empty field.
pub(crate) fn text_field<T: 'static>(
	value: String,
	window: &mut Window,
	cx: &mut Context<T>,
) -> Entity<TextValue> {
	let field = cx.new(|cx| {
		let editor = cx.new(|cx| InputState::new(window, cx));
		TextValue { editor }
	});
	field.update(cx, |field, cx| field.set_value(value, window, cx));
	field
}

/// Keeps a dialog checkbox in sync with its own toggle events.
///
/// A [`CheckBox`] is request-only: it emits [`CheckBoxEvent::Toggled`] carrying
/// the state it *would* move to, and leaves changing itself to its owner. This
/// subscribes to `check` on `cx` and accepts every toggle by writing the
/// proposed state back through [`CheckBox::set_state`], which is how the
/// dialogs in this module react to checkbox clicks without a separate model.
/// The subscription is detached, so it lives as long as `cx` (the owning view);
/// `check` must stay alive at least that long. `T` is the subscribing view type
/// and is not stored.
pub(crate) fn subscribe_checkbox<T: 'static>(check: &Entity<CheckBox>, cx: &mut Context<T>) {
	cx.subscribe(check, |_this, check, event: &CheckBoxEvent, cx| {
		let CheckBoxEvent::Toggled { state, .. } = event;
		check.update(cx, |check, cx| check.set_state(*state, cx));
	})
	.detach();
}

pub(crate) fn rgba_from_array(color: [f64; 4]) -> gpui_kit::Rgba {
	gpui_kit::Rgba {
		r: color[0] as f32,
		g: color[1] as f32,
		b: color[2] as f32,
		a: color[3] as f32,
	}
}

pub(crate) fn color_from_rgba(color: gpui_kit::Rgba) -> [f64; 4] {
	[
		color.r as f64,
		color.g as f64,
		color.b as f64,
		color.a as f64,
	]
}

#[allow(dead_code)]
fn parse_hex_color(text: SharedString) -> Option<[f64; 4]> {
	let text = text.trim().trim_start_matches('#');
	if text.len() != 6 {
		return None;
	}
	let r = u8::from_str_radix(&text[0..2], 16).ok()?;
	let g = u8::from_str_radix(&text[2..4], 16).ok()?;
	let b = u8::from_str_radix(&text[4..6], 16).ok()?;
	Some([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0])
}

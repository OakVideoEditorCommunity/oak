use super::*;

impl<E: AppEngine> OakApp<E> {
	/// Opens the proxy settings dialog (Tools > Proxy Settings; the C++
	/// `ProxyDialog`): the global generation settings plus the footage
	/// proxy list with Generate / Delete buttons.
	pub(crate) fn open_proxy_dialog(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let engine = self.engine.clone();
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::dialogs::ProxyDialogContent::new(engine, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::PROXY,
					ModalOptions::new(crate::i18n::tr("proxydialog.title"), px(560.0))
						.with_button(DialogButton::new(
							crate::i18n::tr("proxydialog.generate"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						))
						.with_button(DialogButton::new(
							crate::i18n::tr("proxydialog.delete"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						))
						.with_button(DialogButton::primary(crate::i18n::tr("proxydialog.close"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::Proxy { modal, content }
		});
	}

	/// Opens the project properties dialog (File > Project Properties…; the
	/// C++ `ProjectPropertiesDialog`): the read-only project name, the
	/// per-project OCIO config override and the disk-cache location. The OK
	/// button applies through the content's `commit` (an invalid OCIO config
	/// keeps the dialog open), Escape / Cancel discard without applying.
	pub(crate) fn open_project_properties(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let engine = self.engine.clone();
		let project_name = self
			.engine
			.read(cx)
			.project()
			.map(|p| p.name.clone())
			.unwrap_or_default();
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::ProjectPropertiesContent::new(engine, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::PROJECT_PROPERTIES,
					ModalOptions::new(
						format!("{} — {project_name}", crate::i18n::tr("projprops.title")),
						px(560.0),
					)
					.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
					.with_button(DialogButton::new(
						crate::i18n::tr("dialog.cancel"),
						crate::widgets::dialog::DialogButtonRole::Secondary,
					)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ProjectProperties { modal, content }
		});
	}

	/// Opens the new-sequence dialog (File > New > Sequence…; the C++
	/// `NewSequenceDialog`): the sequence name plus the format fields. The
	/// OK button creates the sequence through the content's `commit`.
	pub(crate) fn open_new_sequence(&mut self, cx: &mut Context<Self>) {
		self.spawn_new_sequence(SequenceFormatSeed::hd_1080p25(), cx);
	}

	/// Opens the new-sequence dialog seeded from the footage's probed video
	/// parameters (the drop-onto-empty-timeline flow), named like the
	/// footage so the user's OK match the media.
	pub(crate) fn open_new_sequence_for_footage(
		&mut self,
		drop: FootageDropNeedsSequence,
		cx: &mut Context<Self>,
	) {
		let probe = self.engine.read(cx).footage_video_params(drop.footage_id);
		let seed = probe
			.map(|(width, height, rate, interlaced)| {
				SequenceFormatSeed::from_format(
					&crate::oakui::engine::VideoFormat {
						width,
						height,
						rate,
					},
					interlaced,
				)
			})
			.unwrap_or_else(SequenceFormatSeed::hd_1080p25);
		self.spawn_new_sequence(seed, cx);
	}

	/// Opens the new-sequence dialog with the given format seed, while no
	/// other modal is up.
	pub(crate) fn spawn_new_sequence(&mut self, seed: SequenceFormatSeed, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let engine = self.engine.clone();
		self.spawn_modal(cx, move |window, app| {
			let content = app
				.new(|cx| crate::dialogs::NewSequenceContent::new_seeded(engine, seed, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::NEW_SEQUENCE,
					ModalOptions::new(crate::i18n::tr("seqprops.new.title"), px(480.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::NewSequence { modal, content }
		});
	}

	/// A footage drop landed on the timeline while no sequence is open: the
	/// shell pauses it and asks whether to probe the footage's parameters
	/// as the sequence's, or set them up manually.
	pub(crate) fn on_footage_drop_needs_sequence(
		&mut self,
		drop: FootageDropNeedsSequence,
		cx: &mut Context<Self>,
	) {
		if !matches!(self.modal, ModalState::None) {
			// A modal is already up (the drop cannot have been committed
			// against it): resume the drop through the probe path, keeping
			// the engine's auto-create behaviour.
			println!("[shell] drop while a modal is up: using the footage probe params");
			self.engine.update(cx, |engine, cx| {
				let mut at = drop.time;
				for id in drop.footage_ids.iter().copied() {
					engine.drop_footage(id, drop.track_kind, drop.track_index, at, cx);
					let length = engine.footage_length_frames(id).unwrap_or(1).max(1);
					at = Frame(at.0 + length);
				}
			});
			return;
		}
		let probe = self.engine.read(cx).footage_video_params(drop.footage_id);
		self.pending_drop = Some(drop);
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|_cx| crate::dialogs::DropSequenceChoiceContent::new(probe));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::DROP_SEQUENCE_CHOICE,
					ModalOptions::new(crate::i18n::tr("seqprops.drop.title"), px(440.0))
						.with_button(DialogButton::primary(crate::i18n::tr(
							"seqprops.drop.use_footage",
						)))
						.with_button(DialogButton::new(
							crate::i18n::tr("seqprops.drop.manual"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::DropSequenceChoice { modal }
		});
	}

	/// Apply the paused-drop choice: place the clips now, hand off to the
	/// manual new-sequence dialog, or drop nothing.
	///
	/// `choice` comes from the drop-choice dialog's buttons (0 =
	/// [`DropSequenceChoice::UseFootageParams`], 1 =
	/// [`DropSequenceChoice::SpecifyManually`]); `cx` is the entity context.
	/// The paused drop is taken out of `pending_drop` first: if there is none
	/// (a stale dialog event) the modal is just closed.
	///
	/// `UseFootageParams` resumes the drop through the engine's
	/// `drop_footage` path, which sizes the auto-created sequence from the
	/// footage's probed video stream; clips are placed back-to-back starting
	/// at the recorded drop time, each advancing by
	/// `footage_length_frames` (1 frame when unknown, floored at 1). The
	/// dialog then closes. `SpecifyManually` re-stashes the drop and opens
	/// [`Self::open_new_sequence_for_footage`], whose commit resumes it via
	/// [`Self::resume_pending_drop`] — so the drop is only placed after the
	/// new sequence exists. Cancelling the choice dialog is handled as a
	/// dismissal in [`Self::on_modal`], which clears `pending_drop`.
	pub(crate) fn resolve_drop_sequence_choice(
		&mut self,
		choice: DropSequenceChoice,
		cx: &mut Context<Self>,
	) {
		let Some(drop) = self.pending_drop.take() else {
			self.close_modal(cx);
			return;
		};
		match choice {
			DropSequenceChoice::UseFootageParams => {
				// The engine's drop-footage path sizes the new sequence
				// from the footage's probed video stream (the NLE
				// convention); close the choice and place the clip.
				self.engine.update(cx, |engine, cx| {
					let mut at = drop.time;
					for id in drop.footage_ids.iter().copied() {
						engine.drop_footage(id, drop.track_kind, drop.track_index, at, cx);
						let length = engine.footage_length_frames(id).unwrap_or(1).max(1);
						at = Frame(at.0 + length);
					}
				});
				self.close_modal(cx);
			}
			DropSequenceChoice::SpecifyManually => {
				// The manual path re-stashes the drop; the new-sequence
				// dialog's OK resumes it through the same commit below.
				self.pending_drop = Some(drop.clone());
				self.close_modal(cx);
				self.open_new_sequence_for_footage(drop, cx);
			}
		}
	}

	/// The paused drop's last chance to place its clip: called after a
	/// manual new-sequence commit succeeded (the new sequence is open, so
	/// the original floor coordinates still resolve).
	pub(crate) fn resume_pending_drop(&mut self, cx: &mut Context<Self>) {
		let Some(drop) = self.pending_drop.take() else {
			return;
		};
		self.engine.update(cx, |engine, cx| {
			let mut at = drop.time;
			for id in drop.footage_ids.iter().copied() {
				engine.drop_footage(id, drop.track_kind, drop.track_index, at, cx);
				let length = engine.footage_length_frames(id).unwrap_or(1).max(1);
				at = Frame(at.0 + length);
			}
		});
	}

	/// Open the sequence properties dialog for `sequence_id` (the project
	/// explorer's context menu; the C++ `SequencePropertiesDialog`).
	///
	/// Silently does nothing when another modal is already open. The title
	/// carries the sequence's current name (`seqprops.title — <name>`, a
	/// blank name when the engine has no parameters for the id), and the
	/// dialog is constructed with the engine handle and the sequence id so it
	/// can read/write that sequence directly. OK applies the name/format
	/// edits through the content's `commit`: an engine rejection keeps the
	/// dialog open with the error shown, otherwise it closes (see
	/// [`Self::on_modal`]).
	///
	/// `cx` is the entity context; building is deferred through
	/// [`Self::spawn_modal`] and the state is
	/// [`ModalState::SequenceProperties`].
	pub(crate) fn open_sequence_properties(&mut self, sequence_id: u64, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let engine = self.engine.clone();
		let sequence_name = self
			.engine
			.read(cx)
			.sequence_parameters(sequence_id)
			.map(|p| p.name)
			.unwrap_or_default();
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| {
				crate::dialogs::SequencePropertiesContent::new(engine, sequence_id, window, cx)
			});
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::SEQUENCE_PROPERTIES,
					ModalOptions::new(
						format!("{} — {sequence_name}", crate::i18n::tr("seqprops.title")),
						px(480.0),
					)
					.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
					.with_button(DialogButton::new(
						crate::i18n::tr("dialog.cancel"),
						crate::widgets::dialog::DialogButtonRole::Secondary,
					)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::SequenceProperties { modal, content }
		});
	}

	/// Opens the rename dialog for the project-entry `id` (the project
	/// explorer's 重命名 context item), prefilled with the current name.
	pub(crate) fn open_entry_rename(&mut self, id: u64, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let current = self
			.engine
			.read(cx)
			.project_entry_name(id)
			.unwrap_or_default();
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::RenameContent::new(current.clone(), window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::RENAME,
					ModalOptions::new(crate::i18n::tr("project.context.rename"), px(420.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::EntryRename {
				modal,
				content,
				entry_id: id,
			}
		});
	}

	/// Opens the 导出工程文件 dialog: choose the project file format
	/// (OTIO / OVE / FCPXML); the OK button then asks the SYSTEM save
	/// dialog for the output path (the suggested file name carries the
	/// chosen extension).
	pub(crate) fn open_export_project_dfd(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let current = self
			.engine
			.read(cx)
			.project()
			.map(|p| p.path.clone())
			.filter(|p| !p.as_os_str().is_empty());
		let suggested = current
			.and_then(|path| {
				path.file_stem()
					.map(|name| name.to_string_lossy().into_owned())
			})
			.unwrap_or_else(|| "untitled".to_string());
		self.export_suggested_name = suggested;
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::dialogs::ExportProjectDialogContent::new(window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::EXPORT_PROJECT_DFD,
					ModalOptions::new(crate::i18n::tr("menu.file.export_project"), px(480.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ExportProjectDfd { modal, content }
		});
	}

	/// Opens the 另存为 dialog: the copy's new name (prefilled with the
	/// current project name). OK duplicates the project into the library
	/// under that name and switches to the copy.
	pub(crate) fn open_save_as(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let current_name = self
			.engine
			.read(cx)
			.project()
			.map(|p| p.name.clone())
			.unwrap_or_else(|| crate::i18n::tr("manager.new.default_name").to_string());
		let mut name = current_name.clone();
		if let Some((stem, _)) = name.rsplit_once('.') {
			name = stem.to_string();
		}
		name = format!("{name} Copy");
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::RenameContent::new(name.clone().into(), window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::SAVE_AS,
					ModalOptions::new(crate::i18n::tr("menu.file.save_as"), px(420.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::SaveAs { modal, content }
		});
	}

	/// Apply the save-as dialog: duplicate the open project into the library
	/// under the entered name, switch to the copy, then close the dialog.
	///
	/// `content` is the dialog's rename field; its current text is used
	/// verbatim (no trimming and no duplicate-name check here — the library
	/// layer owns naming rules). The engine call `save_project_as(&name)`
	/// decides the result; a failure is only printed (`[file] save as
	/// failed: …`) and the dialog is closed anyway, so the user gets no
	/// in-dialog error and the original project simply stays current.
	///
	/// `cx` is the entity context. Always ends by calling
	/// [`Self::close_modal`].
	pub(crate) fn commit_save_as(
		&mut self,
		content: &Entity<crate::dialogs::RenameContent>,
		cx: &mut Context<Self>,
	) {
		let name = content.read(cx).value(cx).to_string();
		let engine = self.engine.clone();
		let result = engine.update(cx, |engine, cx| engine.save_project_as(&name, cx));
		if let Err(err) = result {
			println!("[file] save as failed: {err}");
		}
		self.close_modal(cx);
	}

	/// Open the subtitle style editor for the first selected timeline clip.
	///
	/// Silently returns when another modal is already open or the timeline
	/// selection is empty. The dialog is seeded from the engine's
	/// `subtitle_style(clip)` (default style when the query fails) and stored
	/// as [`ModalState::SubtitleProperties`]; OK applies the style to that
	/// same clip — the clip is looked up again from the live selection on
	/// commit, so if the selection changed in the meantime the commit fails
	/// with "no clip selected" and the dialog stays open with the message.
	///
	/// `cx` is the entity context; building is deferred through
	/// [`Self::spawn_modal`].
	pub(crate) fn open_subtitle_properties(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let clip = self
			.timeline
			.read(cx)
			.selection()
			.iter()
			.next()
			.map(|clip| clip.0);
		let Some(clip) = clip else {
			return;
		};
		let style = self
			.engine
			.read(cx)
			.subtitle_style(clip)
			.unwrap_or_default();
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::SubtitleStyleContent::new(&style, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::SUBTITLE_PROPERTIES,
					ModalOptions::new(crate::i18n::tr("subtitle.style.title"), px(520.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::SubtitleProperties { modal, content }
		});
	}

	/// Open the audio properties editor for the first selected timeline clip.
	///
	/// Silently returns when another modal is already up or when the timeline
	/// has no selection — the menu item is a no-op rather than an error.
	/// Reads the clip's mix state with `clip_audio` (falling back to defaults
	/// when the clip is not resolvable) and the current sequence's master-bus
	/// chain with `audio_bus_effects`, then builds
	/// [`crate::dialogs::AudioPropertiesContent`] over both. Buttons are OK
	/// (apply, see [`Self::on_modal`]), `audio.props.measure` (button 1,
	/// runs a loudness measurement and displays it) and Cancel.
	///
	/// `cx` is the entity context; the dialog is created through
	/// [`Self::spawn_modal`] and stored as
	/// [`ModalState::AudioProperties`].
	pub(crate) fn open_audio_properties(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let clip = self
			.timeline
			.read(cx)
			.selection()
			.iter()
			.next()
			.map(|clip| clip.0);
		let Some(clip) = clip else {
			return;
		};
		let audio = self.engine.read(cx).clip_audio(clip).unwrap_or_default();
		let bus = self.engine.read(cx).audio_bus_effects();
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::AudioPropertiesContent::new(&audio, &bus, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::AUDIO_PROPERTIES,
					ModalOptions::new(crate::i18n::tr("audio.props.title"), px(520.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("audio.props.measure"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::AudioProperties { modal, content }
		});
	}

	/// Opens the multicam wizard (窗口 → 多机位向导): pick the angle
	/// footage, choose the sync mode, then create the multicam sequence.
	pub(crate) fn open_multicam_wizard(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let engine = self.engine.clone();
		self.spawn_modal(cx, move |window, app| {
			let content =
				app.new(|cx| crate::dialogs::MulticamWizardContent::new(engine, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::MULTICAM_WIZARD,
					ModalOptions::new(crate::i18n::tr("multicam.wizard.title"), px(480.0))
						.with_button(DialogButton::primary(crate::i18n::tr(
							"multicam.wizard.create",
						)))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::MulticamWizard { modal, content }
		});
	}

	/// Apply the multicam wizard: compute per-angle sync offsets, then create
	/// the multicam sequence and close.
	///
	/// `content` is the wizard's dialog state; `cx` is the entity context.
	/// Requires at least two selected angle clips, otherwise returns
	/// `Err(multicam.wizard.need_two)` and the caller keeps the dialog open
	/// with a message row.
	///
	/// Offsets are in seconds, aligned to `selection` order, and depend on
	/// `sync_mode(cx)`: `0` asks the engine for waveform-correlation offsets
	/// (`multicam_wizard_sync_offsets`); if the engine fails (for example when
	/// the reference angle has no decodable audio) — or omits an angle — the
	/// fallback is the source-timecode delta in seconds relative to the first
	/// angle (missing timecode counts as `0`); `1` always uses those
	/// timecode deltas; any other value (no alignment) gives all-zero
	/// offsets, i.e. every angle starts at 0.
	///
	/// Returns the engine's outcome from `multicam_create_sequence(selection,
	/// offsets, name)`, mapped to `()`: `Ok` means the sequence exists (the
	/// caller closes the dialog), `Err(message)` is the engine's rejection
	/// text, displayed in the dialog. Sync fallbacks are silent — a degraded
	/// alignment is not reported as an error.
	pub(crate) fn commit_multicam_wizard(
		&mut self,
		content: &Entity<crate::dialogs::MulticamWizardContent<E>>,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let selection = content.read(cx).selection();
		if selection.len() < 2 {
			return Err(crate::i18n::tr("multicam.wizard.need_two").to_string());
		}
		let sync_mode = content.read(cx).sync_mode(cx);
		let name = content.read(cx).name(cx).to_string();
		// Sync: audio-waveform correlation (0), source timecode (1), or
		// no alignment (2 — all angles start at 0). The engine's sync
		// fails when the reference angle has no decodable audio: the
		// wizard then falls back to source timecode deltas (absolute
		// timestamps, so the first angle is the alignment reference).
		let offsets: Vec<f64> = match sync_mode {
			0 => {
				let synced = self
					.engine
					.read(cx)
					.multicam_wizard_sync_offsets(&selection);
				match synced {
					Ok(offsets) => {
						let reference_tc = selection[0].source_timecode;
						let mut by_id = std::collections::HashMap::new();
						for off in &offsets {
							by_id.insert(off.footage, off.offset_s);
						}
						selection
							.iter()
							.map(|f| {
								by_id.get(&f.id).copied().unwrap_or_else(|| {
									// No audio-sync answer for this angle:
									// the source timecode delta relative to
									// the reference (the fallback).
									let ref_tc = reference_tc.unwrap_or(0);
									f.source_timecode.unwrap_or(0) as f64 / 1000.0
										- ref_tc as f64 / 1000.0
								})
							})
							.collect()
					}
					Err(_) => {
						let reference_tc = selection[0]
							.source_timecode
							.map(|t| t as f64 / 1000.0)
							.unwrap_or(0.0);
						selection
							.iter()
							.map(|f| {
								f.source_timecode.map(|t| t as f64 / 1000.0).unwrap_or(0.0)
									- reference_tc
							})
							.collect()
					}
				}
			}
			1 => {
				let reference_tc = selection[0]
					.source_timecode
					.map(|t| t as f64 / 1000.0)
					.unwrap_or(0.0);
				selection
					.iter()
					.map(|f| {
						f.source_timecode.map(|t| t as f64 / 1000.0).unwrap_or(0.0) - reference_tc
					})
					.collect()
			}
			_ => vec![0.0; selection.len()],
		};
		let result = self.engine.update(cx, |engine, cx| {
			engine.multicam_create_sequence(selection, offsets, name, cx)
		});
		result.map(|_| ())
	}

	/// Open the 导出序列 (export) dialog for one of the project's sequences.
	///
	/// `preselect` is the sequence to select when the dialog is opened from
	/// the project explorer's 导出序列 context item; `None` means "use the
	/// engine's current sequence". The id is validated against the engine's
	/// sequence list, so a stale preselect or a stale current id simply
	/// leaves no entry selected (the dialog then shows its first entry) —
	/// it never selects a sequence that is not in the list. The dialog is
	/// seeded with the full `(id, name)` list, the selection and a default
	/// path from [`Self::default_export_path`].
	///
	/// When the project has no sequences the call only prints
	/// `[export] the project has no sequences` and leaves the UI untouched
	/// (no empty dialog). Building is deferred through
	/// [`Self::spawn_modal`]; the resulting state is
	/// [`ModalState::Export`], with the primary button running
	/// [`Self::begin_export`] and the secondary cancelling.
	pub(crate) fn open_export_dialog(&mut self, preselect: Option<u64>, cx: &mut Context<Self>) {
		let sequences = self.engine.read(cx).sequence_entries();
		if sequences.is_empty() {
			println!("[export] the project has no sequences");
			return;
		}
		let selected = preselect
			.or_else(|| self.engine.read(cx).current_sequence_id())
			.filter(|id| sequences.iter().any(|(entry, _)| entry == id));
		let default_path = self.default_export_path(cx, &sequences, selected);
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| ExportDialogContent::new(window, cx));
			content.update(app, |content, cx| {
				content.set_sequences(sequences, selected, cx);
				content.set_path(default_path.clone(), window, cx)
			});
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::EXPORT,
					ModalOptions::new(crate::i18n::tr("export.title"), px(440.0))
						.with_button(DialogButton::primary(crate::i18n::tr("export.run")))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::Export { modal, content }
		});
	}

	/// Suggest an output path for the export dialog: `<sequence>.mp4` next to
	/// the project file.
	///
	/// `sequences` is the engine's `(sequence id, display name)` list and
	/// `selected` the id currently chosen in the dialog (or `None` for
	/// "first entry"). The base name is the selected sequence's name, else
	/// the first sequence's, else the project's name, else `"untitled"`;
	/// empty names are skipped at each step, so a blank sequence label falls
	/// through to the project name. The directory is the project file's
	/// parent, or `.` when no project is open (i.e. the process working
	/// directory), and the result is a lossy UTF-8 string.
	///
	/// The `.mp4` suffix is hardcoded here and does **not** follow the
	/// format chosen in the dialog; it is only a default the user can edit,
	/// and [`Self::begin_export`] appends the real format extension when the
	/// path has none. `cx` is the app context used to read the engine's
	/// project handle.
	pub(crate) fn default_export_path(
		&self,
		cx: &App,
		sequences: &[(u64, SharedString)],
		selected: Option<u64>,
	) -> String {
		let seq_name = selected
			.and_then(|id| sequences.iter().find(|(entry, _)| *entry == id))
			.or_else(|| sequences.first())
			.map(|(_, name)| name.to_string());
		let project = self.engine.read(cx).project();
		let name = seq_name
			.filter(|n| !n.is_empty())
			.or_else(|| project.map(|p| p.name.clone()).filter(|n| !n.is_empty()))
			.unwrap_or_else(|| "untitled".to_string());
		let dir = project
			.and_then(|p| p.path.parent().map(|d| d.to_path_buf()))
			.unwrap_or_else(|| PathBuf::from("."));
		dir.join(format!("{name}.mp4"))
			.to_string_lossy()
			.into_owned()
	}

	/// Start the export configured in the export dialog and replace that
	/// dialog with the progress dialog.
	///
	/// Only runs while `self.modal` is [`ModalState::Export`]; any other
	/// state makes it a no-op. Reads the dialog's settings, container
	/// extension, output path and selected sequence; an all-whitespace path
	/// or a missing selection returns without changing anything, leaving the
	/// dialog up for the user to fix it. When the typed path has no
	/// non-empty extension the format's extension is appended
	/// (`path + "." + ext`) — an existing extension is respected as typed,
	/// even if it disagrees with the chosen format.
	///
	/// The engine call `start_export_of(sequence, &settings, path)` decides
	/// the two outcomes: on success the returned session is stored in
	/// `self.export` (so [`Self::poll_export`] can drain progress and
	/// [`Self::cancel_export`] can abort) and the modal is swapped for the
	/// progress dialog; on failure the error is printed to stdout and the
	/// dialog is simply closed — the UI shows no failure message, so a start
	/// error is only visible in the log.
	pub(crate) fn begin_export(&mut self, cx: &mut Context<Self>) {
		let ModalState::Export { content, .. } = &self.modal else {
			return;
		};
		let settings = content.read(cx).settings(cx);
		let ext = content.read(cx).extension(cx);
		let mut path = content.read(cx).path(cx).to_string();
		if path.trim().is_empty() {
			return;
		}
		let Some(sequence) = content.read(cx).selected_sequence(cx) else {
			return;
		};
		// Append the format's extension when the user left it off.
		let has_ext = std::path::Path::new(&path)
			.extension()
			.map(|e| !e.to_string_lossy().is_empty())
			.unwrap_or(false);
		if !has_ext {
			path = format!("{path}.{ext}");
		}

		let result = self.engine.update(cx, |engine, _cx| {
			engine.start_export_of(sequence, &settings, PathBuf::from(&path))
		});
		match result {
			Ok(session) => {
				self.export = Some(ExportRun { session });
				self.spawn_modal(cx, |window, app| {
					let (modal, content) = progress_dialog(
						modal_ids::EXPORT_PROGRESS,
						crate::i18n::tr("export.progress.title"),
						crate::i18n::tr("export.progress.label"),
						window,
						app,
					);
					ModalState::Progress { modal, content }
				});
			}
			Err(err) => {
				println!("[export] failed to start: {err}");
				self.close_modal(cx);
			}
		}
	}

	/// Cancels the running export (the task aborts at the next frame).
	pub(crate) fn cancel_export(&mut self, cx: &mut Context<Self>) {
		if let Some(run) = &self.export {
			(run.session.cancel)();
		}
		let _ = cx;
	}

	/// Drains the export progress events on the tick loop: updates the
	/// progress bar and closes the dialog when the task finishes.
	pub(crate) fn poll_export(&mut self, cx: &mut Context<Self>) {
		let Some(run) = &self.export else {
			return;
		};
		let mut events = Vec::new();
		while let Ok(event) = run.session.events.try_recv() {
			events.push(event);
		}
		if events.is_empty() {
			return;
		}
		let mut finished: Option<(bool, String)> = None;
		for event in events {
			match event {
				crate::oakui::ExportEvent::Started => {}
				crate::oakui::ExportEvent::Progress(fraction) => {
					if let ModalState::Progress { content, .. } = &self.modal {
						let fraction = fraction as f32;
						content.update(cx, |content, cx| content.set_progress(fraction, cx));
					}
				}
				crate::oakui::ExportEvent::Finished(ok, err) => finished = Some((ok, err)),
			}
		}
		if let Some((ok, err)) = finished {
			self.export = None;
			self.modal = ModalState::None;
			if ok {
				println!("[export] finished");
			} else {
				println!("[export] failed: {err}");
			}
			cx.notify();
		}
	}
}

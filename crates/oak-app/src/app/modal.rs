use super::*;

impl<E: AppEngine> OakApp<E> {
	/// Close the current modal and repaint the shell.
	///
	/// Sets `self.modal` to [`ModalState::None`], calls `cx.notify()`, then
	/// opens any update prompt that was deferred while a modal was up
	/// (`pending_update`, e.g. a startup update check completing behind the
	/// project manager) via [`Self::open_update_available`]. Safe to call when
	/// no modal is open: it is just a repaint plus the deferred-prompt check.
	/// Closing a manager sub-dialog this way does *not* return to the manager;
	/// [`Self::back_to_manager`] does that.
	///
	/// `cx` is the entity context. Callers that resume background work
	/// (export, drop) rely on the modal state being cleared before the next
	/// event, which this guarantees.
	pub fn close_modal(&mut self, cx: &mut Context<Self>) {
		self.modal = ModalState::None;
		cx.notify();
		// A checked update prompt deferred while another modal was up (the
		// project manager at startup) opens now that the layer is free.
		if let Some(release) = self.pending_update.take() {
			self.open_update_available(release, cx);
		}
	}

	/// Builds a modal on the main window, subscribes it to
	/// [`Self::on_modal`] and layers it onto the shell.
	///
	/// The modal is created inside `update_window` (modal widgets need a
	/// `&mut Window`); the state swap and the subscription happen *after* the
	/// window update returns, on this entity's own `Context` — swapping state
	/// through a weak handle *inside* the window callback would re-enter this
	/// entity while it is already being updated (the crash seen when opening
	/// Preferences from a menu action).
	///
	/// The caller must NOT be inside a window update itself (e.g.
	/// `WindowHandle::update`): the nested `update_window` below would fail
	/// and the modal would silently not open. Drive the root entity instead
	/// (menu actions and entity updates are fine).
	/// Builds and installs a modal. When the caller is already inside a
	/// window update (an action listener / menu dispatch / tick), a nested
	/// `update_window` would silently fail and drop the dialog — the phase-7
	/// fix for Preferences / Action Search, applied centrally: probe first,
	/// and defer the build to the end of the current app update when nested.
	/// The fast path stays synchronous so callers can rely on `self.modal`
	/// right after the call.
	pub(crate) fn spawn_modal(
		&mut self,
		cx: &mut Context<Self>,
		build: impl FnOnce(&mut Window, &mut App) -> ModalState<E> + 'static,
	) {
		let handle = cx.windows().first().copied();
		let nested = match handle {
			Some(handle) => cx.update_window(handle, |_root, _window, _app| ()).is_err(),
			None => false,
		};
		if nested {
			let this = cx.weak_entity();
			cx.defer(move |app| {
				let _ = this.update(app, |this, cx| this.spawn_modal_now(cx, build));
			});
			return;
		}
		self.spawn_modal_now(cx, build);
	}

	/// Build and install a modal immediately (the non-deferred half of
	/// [`Self::spawn_modal`]).
	///
	/// `build` is called inside a window update so modal widgets get their
	/// `&mut Window`; its resulting [`ModalState`] is moved into `self.modal`
	/// (the swap happens outside the window callback, on this entity's own
	/// context, to avoid re-entering the entity) and the modal entity is
	/// subscribed to [`Self::on_modal`], then `cx.notify()` repaints.
	///
	/// Becomes a silent no-op with no windows yet, when the `update_window`
	/// call fails (the caller is inside a window update — use
	/// [`Self::spawn_modal`] instead, which probes and defers), or if the
	/// built state somehow carries no modal entity (the
	/// `expect("spawned modal always carries a Modal")` then panics).
	///
	/// The fast path is synchronous: after a successful call, `self.modal`
	/// already holds the new dialog, which callers rely on.
	pub(crate) fn spawn_modal_now(
		&mut self,
		cx: &mut Context<Self>,
		build: impl FnOnce(&mut Window, &mut App) -> ModalState<E>,
	) {
		let windows = cx.windows();
		let Some(handle) = windows.first() else {
			return;
		};
		let Ok(state) = cx.update_window(*handle, |_root, window, app| build(window, app)) else {
			return;
		};
		let modal = state
			.modal_entity()
			.expect("spawned modal always carries a Modal");
		cx.subscribe(&modal, |this, _entity, event: &ModalEvent, cx| {
			this.on_modal(event, cx);
		})
		.detach();
		self.modal = state;
		cx.notify();
	}

	/// Apply one Sequence > Audio menu operation to every selected clip.
	///
	/// `edit` selects the engine call: `Gain(factor)` sets the audio gain
	/// (`1.0` = unity, `0.0` = silence, linear — the menu offers 0.5 / 1.0 /
	/// 2.0), `Pan(value)` sets constant-power pan in `-1.0` (hard left) ..
	/// `1.0` (hard right) with `0.0` centre, `FadeIn`/`FadeOut` set the
	/// clip's fades to a 1-second ramp on that side while clearing the other
	/// (`(1,1)` vs `(0,1)` rational seconds, `(0,1)` meaning "no fade"), and
	/// `AddVolume`/`AddPan` append the built-in volume/pan node to each clip's
	/// audio effect chain.
	///
	/// `cx` is the entity context. Does nothing when the timeline selection
	/// is empty. Failures (no project, a rejected clip, or an engine without
	/// clip-audio support) are printed as `[audio] <edit> failed: …` after
	/// the clips already written by that call stayed written — there is no
	/// error dialog and no rollback.
	pub(crate) fn edit_selected_audio(&mut self, edit: AudioEdit, cx: &mut Context<Self>) {
		let clips: Vec<u64> = self
			.timeline
			.read(cx)
			.selection()
			.iter()
			.map(|clip| clip.0)
			.collect();
		if clips.is_empty() {
			return;
		}
		let result = self.engine.update(cx, |engine, cx| match edit {
			AudioEdit::Gain(gain) => engine.set_clip_audio_gain(&clips, gain, cx),
			AudioEdit::Pan(pan) => engine.set_clip_audio_pan(&clips, pan, cx),
			AudioEdit::FadeIn => engine.set_clip_audio_fade(&clips, (1, 1), (0, 1), cx),
			AudioEdit::FadeOut => engine.set_clip_audio_fade(&clips, (0, 1), (1, 1), cx),
			AudioEdit::AddVolume => {
				engine.add_clip_audio_effect(&clips, "org.olivevideoeditor.Olive.volume", cx)
			}
			AudioEdit::AddPan => {
				engine.add_clip_audio_effect(&clips, "org.olivevideoeditor.Olive.pan", cx)
			}
		});
		if let Err(err) = result {
			println!("[audio] {edit:?} failed: {err}");
		}
	}

	/// Opens the platform file dialog for `action` and routes the picked
	/// path(s) through the engine. Open / Import use the path picker (import
	/// footage allows multiple files); the 导出工程文件… and the manager's
	/// export ask for a new path. The picker resolves asynchronously, so the
	/// chosen path is applied in a spawned task via [`Self::on_file_paths`].
	pub(crate) fn open_file_dialog(&mut self, action: FileAction, cx: &mut Context<Self>) {
		match action {
			FileAction::Open
			| FileAction::ImportFootage
			| FileAction::ImportProject
			| FileAction::ImportSubtitles => {
				let prompt = match action {
					FileAction::Open => crate::i18n::tr("file.open.title"),
					FileAction::ImportProject => crate::i18n::tr("manager.import.title"),
					FileAction::ImportSubtitles => crate::i18n::tr("file.import_subtitles.title"),
					_ => crate::i18n::tr("file.import_footage.title"),
				};
				let receiver = cx.prompt_for_paths(PathPromptOptions {
					files: true,
					directories: false,
					multiple: matches!(
						action,
						FileAction::ImportFootage | FileAction::ImportSubtitles
					),
					prompt: Some(prompt.into()),
				});
				cx.spawn(async move |this, cx| {
					if let Ok(Ok(Some(paths))) = receiver.await {
						if !paths.is_empty() {
							let _ =
								this.update(cx, |this, cx| this.on_file_paths(action, paths, cx));
						}
					}
				})
				.detach();
			}
			FileAction::ExportProjectFile | FileAction::ExportSubtitles => {
				let current = self
					.engine
					.read(cx)
					.project()
					.map(|p| p.path.clone())
					.filter(|p| !p.as_os_str().is_empty());
				let (directory, suggested) = match current {
					Some(path) => (
						path.parent()
							.map(|dir| dir.to_path_buf())
							.unwrap_or_else(|| PathBuf::from(".")),
						path.file_name()
							.map(|name| name.to_string_lossy().into_owned()),
					),
					None => (PathBuf::from("."), None),
				};
				let receiver = cx.prompt_for_new_path(&directory, suggested.as_deref());
				cx.spawn(async move |this, cx| {
					if let Ok(Ok(Some(path))) = receiver.await {
						let _ = this.update(cx, |this, cx| {
							this.on_file_paths(action, vec![path], cx);
						});
					}
				})
				.detach();
			}
			// The manager's export prompts in `open_manager_export` (it needs
			// the selection's suggested filename).
			FileAction::ExportProject => {}
		}
	}

	/// Deliver paths chosen in a platform file dialog to the engine according
	/// to `action`.
	///
	/// `paths` are the picked files (`Vec` because footage import accepts
	/// several; every other action only reads the first entry, so extra
	/// selections are ignored). `cx` is the entity context; the engine call
	/// runs inside one `engine.update`, so the whole batch is one app update.
	///
	/// Routing: `Open` / `ExportProjectFile` / `ImportSubtitles` /
	/// `ExportSubtitles` take `paths.first()` (no path = no-op `Ok`);
	/// `ImportFootage` imports all of them in order and reports the **first**
	/// failure after attempting the rest. The manager-only actions are
	/// handled first and return early: `ImportProject` imports into the
	/// project library and reloads the manager list on success
	/// ([`Self::reload_manager`]), `ExportProject` consumes the uuid stashed
	/// in `pending_export` and exports that project to the first path — both
	/// report failures through [`Self::manager_status`] (the manager's status
	/// line) rather than the log.
	///
	/// Failures on the remaining actions are printed as
	/// `[file] <action> failed: …`; there is no error dialog. An empty
	/// `paths` for a single-path action is a silent no-op.
	pub(crate) fn on_file_paths(
		&mut self,
		action: FileAction,
		paths: Vec<PathBuf>,
		cx: &mut Context<Self>,
	) {
		// The manager's library import/export refresh the open manager and
		// report failures in its status line.
		match action {
			FileAction::ImportProject => {
				let Some(path) = paths.first() else {
					return;
				};
				let result = self.engine.update(cx, |engine, _cx| {
					engine.library_import_project(path.clone())
				});
				match result {
					Ok(uuid) => {
						println!("[manager] imported \"{}\" as {uuid}", path.display());
						self.reload_manager(cx);
					}
					Err(err) => self.manager_status(err, cx),
				}
				return;
			}
			FileAction::ExportProject => {
				let (Some(uuid), Some(path)) = (self.pending_export.take(), paths.first()) else {
					return;
				};
				let result = self.engine.update(cx, |engine, _cx| {
					engine.library_export_project(&uuid, path.clone())
				});
				match result {
					Ok(()) => println!("[manager] exported to \"{}\"", path.display()),
					Err(err) => self.manager_status(err, cx),
				}
				return;
			}
			_ => {}
		}
		let result = self.engine.update(cx, |engine, cx| match action {
			FileAction::Open => match paths.first() {
				Some(path) => engine.open_project_path(path.clone(), cx),
				None => Ok(()),
			},
			FileAction::ExportProjectFile => match paths.first() {
				Some(path) => engine.export_project_path(path.clone(), cx),
				None => Ok(()),
			},
			FileAction::ImportSubtitles => match paths.first() {
				Some(path) => engine.import_subtitles(path.clone(), cx).map(|_| ()),
				None => Ok(()),
			},
			FileAction::ExportSubtitles => match paths.first() {
				Some(path) => engine.export_subtitles(path.clone(), cx).map(|_| ()),
				None => Ok(()),
			},
			FileAction::ImportFootage => {
				// Import accepts several files at once; keep the first failure
				// for the log after the rest have been attempted.
				let mut first_error = None;
				for path in paths {
					if let Err(err) = engine.import_footage(path.clone(), cx) {
						first_error.get_or_insert(err);
					}
				}
				match first_error {
					Some(err) => Err(err),
					None => Ok(()),
				}
			}
			// Handled above (the manager's library import/export).
			FileAction::ImportProject | FileAction::ExportProject => Ok(()),
		});
		if let Err(err) = result {
			println!("[file] {action:?} failed: {err}");
		}
	}
}

impl<E: AppEngine> OakApp<E> {
	/// Route one modal-dialog event: the single dispatch point from dialog
	/// widgets back into app state.
	///
	/// `event` is either [`ModalEvent::ButtonClicked`] (a button index per
	/// dialog's wiring) or [`ModalEvent::Dismissed`] (Escape or backdrop
	/// click); it is matched against the `control` id in `modal_ids`, so a
	/// control id that no longer matches the active [`ModalState`] is inert.
	/// `cx` is the entity context used to read dialog content, drive the
	/// engine and repaint.
	///
	/// Button semantics follow each dialog's contract: index `0` is the
	/// primary ("OK"-style) action and the rest cancel/close, with notable
	/// exceptions — export progress only acts on cancel (button 1) and the
	/// finished event closes it; the proxy dialog's buttons are
	/// generate/delete/accept; audio properties button 1 measures loudness.
	/// Primary actions that the engine can reject keep the dialog open with
	/// the error pushed into the dialog (project properties, sequence
	/// properties, subtitle properties, multicam wizard), while the others
	/// close unconditionally (clip rename/speed). Dismissal is treated as
	/// cancel: it aborts a running export, returns from manager sub-dialogs
	/// to the manager, commits preferences, and abandons a paused drag-drop
	/// for the drop-choice/new-sequence dialogs. All dialog mutations happen
	/// through `self.engine.update`, so they are recorded by the active
	/// engine implementation.
	pub(crate) fn on_modal(&mut self, event: &ModalEvent, cx: &mut Context<Self>) {
		match event {
			ModalEvent::ButtonClicked { control, button } => match *control {
				modal_ids::EXPORT => {
					if *button == 0 {
						self.begin_export(cx);
					} else {
						self.close_modal(cx);
					}
				}
				modal_ids::EXPORT_PROGRESS => {
					if *button == 1 {
						// Cancel button: ask the task to abort; the finished
						// event closes the dialog.
						self.cancel_export(cx);
					}
				}
				modal_ids::PLUGIN_PROGRESS => {
					if *button == 1 {
						// Cancel the plugin render; the reporter then answers
						// false and the render aborts at the next progress
						// update (or the dialog closes on the 1.0 fraction).
						crate::oakui::ofx::cancel_plugin_render();
						self.plugin_progress_open = false;
						self.close_modal(cx);
					}
				}
				modal_ids::PREFERENCES => {
					self.commit_preferences(cx);
					self.close_modal(cx);
				}
				modal_ids::MANAGER => self.close_modal(cx),
				modal_ids::MANAGER_RENAME => {
					if *button == 0 {
						self.confirm_manager_rename(cx);
					} else {
						self.back_to_manager(cx);
					}
				}
				modal_ids::MANAGER_DELETE => {
					if *button == 0 {
						self.confirm_manager_delete(cx);
					} else {
						self.back_to_manager(cx);
					}
				}
				modal_ids::CLIP_RENAME => {
					if *button == 0 {
						if let ModalState::ClipRename { content, clips, .. } = &self.modal {
							let name = content.read(cx).value(cx);
							let clips = clips.clone();
							if !name.is_empty() {
								self.engine
									.update(cx, |engine, cx| engine.rename_clips(clips, name, cx));
							}
						}
					}
					self.close_modal(cx);
				}
				modal_ids::CLIP_SPEED => {
					if *button == 0 {
						if let ModalState::ClipSpeed { content, clips, .. } = &self.modal {
							let text = content.read(cx).value(cx);
							// Accept "2", "2.0", "200%" — a percent sign
							// divides by 100.
							let parsed = text
								.strip_suffix('%')
								.and_then(|p| p.trim().parse::<f64>().ok())
								.map(|p| p / 100.0)
								.or_else(|| text.parse::<f64>().ok());
							if let Some(speed) = parsed.filter(|v| v.is_finite() && *v > 0.0) {
								let clips = clips.clone();
								self.engine.update(cx, |engine, cx| {
									engine.set_clips_speed(clips, speed, cx)
								});
							}
						}
					}
					self.close_modal(cx);
				}
				modal_ids::PROXY => {
					if let ModalState::Proxy { content, .. } = &self.modal {
						let content = content.clone();
						match *button {
							0 => content.update(cx, |dialog, cx| dialog.generate(cx)),
							1 => content.update(cx, |dialog, cx| dialog.delete(cx)),
							_ => {
								content.update(cx, |dialog, cx| dialog.accept(cx));
								self.close_modal(cx);
							}
						}
					}
				}
				modal_ids::ABOUT => self.close_modal(cx),
				modal_ids::UPDATE_AVAILABLE => {
					// Primary opens the downloads page in the user's
					// browser; the secondary (and the mask) just dismisses.
					if *button == 0 {
						cx.open_url(crate::update::DOWNLOAD_URL);
					}
					self.close_modal(cx);
				}
				modal_ids::PROJECT_PROPERTIES => {
					if let ModalState::ProjectProperties { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 0 {
							// OK: apply the settings; an invalid OCIO config
							// keeps the dialog open with the error shown.
							match content.update(cx, |dialog, cx| dialog.commit(cx)) {
								Ok(()) => self.close_modal(cx),
								Err(err) => {
									content
										.update(cx, |dialog, cx| dialog.set_error(Some(err), cx));
								}
							}
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::SUBTITLE_PROPERTIES => {
					if let ModalState::SubtitleProperties { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 0 {
							let style = content.read(cx).style(cx);
							match style.and_then(|style| {
								let Some(clip) = self
									.timeline
									.read(cx)
									.selection()
									.iter()
									.next()
									.map(|clip| clip.0)
								else {
									return Err("no clip selected".to_string());
								};
								self.engine.update(cx, |engine, cx| {
									engine.set_subtitle_style(clip, &style, cx)
								})
							}) {
								Ok(()) => self.close_modal(cx),
								Err(err) => {
									content
										.update(cx, |dialog, cx| dialog.set_error(Some(err), cx));
								}
							}
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::AUDIO_PROPERTIES => {
					if let ModalState::AudioProperties { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 1 {
							let loudness = self
								.engine
								.update(cx, |engine, _cx| engine.measure_loudness());
							content.update(cx, |dialog, cx| dialog.set_loudness(loudness, cx));
						} else if *button == 0 {
							let Some(clip) = self
								.timeline
								.read(cx)
								.selection()
								.iter()
								.next()
								.map(|clip| clip.0)
							else {
								return;
							};
							let (gain, pan, fade_in, fade_out) = content.read(cx).mix(cx);
							let initial = content.read(cx).initial_effects().to_vec();
							let desired = content.read(cx).effects(cx);
							let mut used = vec![false; initial.len()];
							let mut updates: Vec<(usize, Vec<(String, f64)>)> = Vec::new();
							let mut additions: Vec<(String, Vec<(String, f64)>)> = Vec::new();
							for (type_id, params) in &desired {
								if let Some(position) = initial
									.iter()
									.enumerate()
									.find(|(index, effect)| {
										!used[*index] && effect.type_id == *type_id
									})
									.map(|(index, _)| index)
								{
									used[position] = true;
									updates.push((position, params.clone()));
								} else {
									additions.push((type_id.clone(), params.clone()));
								}
							}
							let removals: Vec<usize> = initial
								.iter()
								.enumerate()
								.filter_map(|(index, _)| (!used[index]).then_some(index))
								.collect();
							let final_len = initial.len() - removals.len();
							let result = self.engine.update(cx, |engine, cx| {
								engine.set_clip_audio_gain(&[clip], gain, cx)?;
								engine.set_clip_audio_pan(&[clip], pan, cx)?;
								engine.set_clip_audio_fade(
									&[clip],
									((fade_in * 1000.0) as i64, 1000),
									((fade_out * 1000.0) as i64, 1000),
									cx,
								)?;
								for (index, params) in &updates {
									for (input, value) in params {
										engine.set_clip_audio_effect_param(
											clip, *index, input, *value, cx,
										)?;
									}
									if !initial[*index].enabled {
										engine.set_clip_audio_effect_enabled(
											clip, *index, true, cx,
										)?;
									}
								}
								for index in removals.iter().rev() {
									engine.remove_clip_audio_effect(clip, *index, cx)?;
								}
								for (offset, (type_id, params)) in additions.iter().enumerate() {
									engine.add_clip_audio_effect(&[clip], type_id, cx)?;
									for (input, value) in params {
										engine.set_clip_audio_effect_param(
											clip,
											final_len + offset,
											input,
											*value,
											cx,
										)?;
									}
								}
								let (bus_enabled, bus_ceiling, bus_release) =
									content.read(cx).bus_limiter(cx);
								engine.set_audio_bus_limiter(
									bus_enabled,
									bus_ceiling,
									bus_release,
									cx,
								)?;
								Ok(())
							});
							match result {
								Ok(()) => self.close_modal(cx),
								Err(err) => {
									content.update(cx, |dialog, cx| dialog.set_error(Some(err), cx))
								}
							}
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::NEW_SEQUENCE => {
					if let ModalState::NewSequence { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 0 {
							// OK: create the sequence; a rejected create
							// keeps the dialog open with the error shown.
							match content.update(cx, |dialog, cx| dialog.commit(cx)) {
								Ok(()) => {
									self.close_modal(cx);
									// The manual path of the drop-onto-empty-
									// timeline flow resumes the paused drop
									// now that a sequence is open.
									self.resume_pending_drop(cx);
								}
								Err(err) => {
									content
										.update(cx, |dialog, cx| dialog.set_error(Some(err), cx));
								}
							}
						} else {
							// Cancel: any paused drop is abandoned too.
							self.pending_drop = None;
							self.close_modal(cx);
						}
					}
				}
				modal_ids::DROP_SEQUENCE_CHOICE => {
					// 0 = handle it from the footage probe, 1 = manual
					// dialog, 2 = cancel.
					match *button {
						0 => self
							.resolve_drop_sequence_choice(DropSequenceChoice::UseFootageParams, cx),
						1 => self
							.resolve_drop_sequence_choice(DropSequenceChoice::SpecifyManually, cx),
						_ => {
							// Cancel: abandon the paused drop.
							self.pending_drop = None;
							self.close_modal(cx);
						}
					}
				}
				modal_ids::MULTICAM_WIZARD => {
					if let ModalState::MulticamWizard { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 0 {
							// OK: create the multicam; a rejected create
							// keeps the dialog open.
							match self.commit_multicam_wizard(&content, cx) {
								Ok(()) => self.close_modal(cx),
								Err(err) => {
									println!("[multicam wizard] {err}");
								}
							}
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::RENAME => {
					if let ModalState::EntryRename {
						content, entry_id, ..
					} = &self.modal
					{
						if *button == 0 {
							// OK: apply the new name.
							let name = content.read(cx).value(cx);
							let id = *entry_id;
							let engine = self.engine.clone();
							engine.update(cx, |engine, cx| {
								engine.rename_entry(id, name.to_string(), cx)
							});
							self.close_modal(cx);
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::EXPORT_PROJECT_DFD => {
					if let ModalState::ExportProjectDfd { content, .. } = &self.modal {
						if *button == 0 {
							// OK: keep the chosen format; the SYSTEM save
							// dialog then picks the output path (the
							// suggested name carries the format's
							// extension).
							let format = content.read(cx).format(cx);
							let ext = match format {
								crate::dialogs::PROJECT_FORMAT_OVE => "ove",
								crate::dialogs::PROJECT_FORMAT_FCPXML => "fcpxml",
								_ => "otio",
							};
							let suggested = self.export_suggested_name.clone();
							let receiver = cx.prompt_for_new_path(
								std::path::Path::new("."),
								Some(&format!("{suggested}.{ext}")),
							);
							self.close_modal(cx);
							cx.spawn(async move |this, cx| {
								if let Ok(Ok(Some(path))) = receiver.await {
									let _ = this.update(cx, |this, cx| {
										let engine = this.engine.clone();
										let _ = engine.update(cx, |engine, cx| {
											engine.export_project_path(path, cx)
										});
									});
								}
							})
							.detach();
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::NEW_PROJECT => {
					if let ModalState::NewProject { content, .. } = &self.modal {
						if *button == 0 {
							let content = content.clone();
							self.commit_new_project(&content, cx);
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::SAVE_AS => {
					if let ModalState::SaveAs { content, .. } = &self.modal {
						if *button == 0 {
							let content = content.clone();
							self.commit_save_as(&content, cx);
						} else {
							self.close_modal(cx);
						}
					}
				}
				modal_ids::SEQUENCE_PROPERTIES => {
					if let ModalState::SequenceProperties { content, .. } = &self.modal {
						let content = content.clone();
						if *button == 0 {
							// OK: apply the edits; a rejected update keeps
							// the dialog open with the error shown.
							match content.update(cx, |dialog, cx| dialog.commit(cx)) {
								Ok(()) => self.close_modal(cx),
								Err(err) => {
									content
										.update(cx, |dialog, cx| dialog.set_error(Some(err), cx));
								}
							}
						} else {
							self.close_modal(cx);
						}
					}
				}
				_ => {}
			},
			ModalEvent::Dismissed { control } => match *control {
				modal_ids::EXPORT_PROGRESS => {
					// Escape cancels the running export and closes the dialog.
					self.cancel_export(cx);
					self.close_modal(cx);
				}
				// Escape from a manager sub-dialog returns to the manager.
				modal_ids::MANAGER_RENAME | modal_ids::MANAGER_DELETE => {
					self.back_to_manager(cx);
				}
				modal_ids::PREFERENCES => {
					self.commit_preferences(cx);
					self.close_modal(cx);
				}
				// Escape closes the proxy dialog without applying (the
				// Close button is the apply path, like the C++ accept()).
				modal_ids::PROXY => self.close_modal(cx),
				// Escape / backdrop close the project properties dialog
				// without applying (only OK commits).
				modal_ids::PROJECT_PROPERTIES => self.close_modal(cx),
				// Escape from the drop choice (or the manual new-sequence
				// dialog it spawned) abandons the paused drop.
				modal_ids::DROP_SEQUENCE_CHOICE | modal_ids::NEW_SEQUENCE => {
					self.pending_drop = None;
					self.close_modal(cx);
				}
				_ => self.close_modal(cx),
			},
		}
	}
}

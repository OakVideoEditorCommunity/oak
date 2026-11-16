use super::*;

impl<E: AppEngine> OakApp<E> {
	/// Opens the preferences dialog (General + Keyboard tabs). Theme/language
	/// selections emit [`crate::dialogs::PreferencesEvent`]s, applied to the
	/// shell chrome immediately; the typed cache directory commits when the
	/// dialog closes; a shortcut change re-binds the key map and rebuilds the
	/// menu bar, so the new keys are live without a restart.
	///
	/// The build is deferred to the end of the current app update (like
	/// [`Self::open_action_search`]): the dialog is usually opened from a
	/// shortcut or a menu click, both of which dispatch inside a window
	/// update where `spawn_modal`'s nested `update_window` would silently
	/// fail.
	pub fn open_preferences(&mut self, cx: &mut Context<Self>) {
		let weak = cx.weak_entity();
		cx.defer(move |app| {
			if let Some(this) = weak.upgrade() {
				this.update(app, |this, cx| this.open_preferences_now(cx));
			}
		});
	}

	/// The deferred half of [`Self::open_preferences`]: builds the tabbed
	/// dialog and subscribes to its [`crate::dialogs::PreferencesEvent`]s.
	pub(crate) fn open_preferences_now(&mut self, cx: &mut Context<Self>) {
		self.spawn_modal(cx, |window, app| {
			let content = app.new(|cx| PreferencesDialogContent::new(window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::PREFERENCES,
					ModalOptions::new(crate::i18n::tr("preferences.title"), px(720.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.close"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::Preferences { modal, content }
		});
		if let ModalState::Preferences { content, .. } = &self.modal {
			let content = content.clone();
			cx.subscribe(
				&content,
				|this, _content, event: &crate::dialogs::PreferencesEvent, cx| match *event {
					crate::dialogs::PreferencesEvent::ThemeChanged(dark) => {
						this.apply_dark(dark, cx);
					}
					crate::dialogs::PreferencesEvent::LanguageChanged => {
						// The gpui-kit dock renders panel titles live, so a
						// language switch needs no title refresh.
						this.rebuild_menu_bar(cx);
						// The dialog's own labels are localized: repaint it too.
						_content.update(cx, |_dialog, cx| cx.notify());
						cx.notify();
					}
					crate::dialogs::PreferencesEvent::ShortcutsChanged => {
						this.rebind_keys(cx);
					}
					crate::dialogs::PreferencesEvent::DisplayColorChanged => {
						this.reapply_display_color(cx);
					}
				},
			)
			.detach();
		}
	}

	/// Commits the preferences dialog's free-text fields (the cache
	/// directory path) before the modal closes.
	pub(crate) fn commit_preferences(&mut self, cx: &mut Context<Self>) {
		if let ModalState::Preferences { content, .. } = &self.modal {
			content.update(cx, |content, cx| content.commit_cache_dir(cx));
		}
		// The committed custom-ICC path may change the effective display
		// transform (or the policy, if the profile availability flipped):
		// re-declare on every window.
		self.reapply_display_color(cx);
	}

	/// Re-applies the global key bindings and rebuilds the menu bar after a
	/// shortcut override change. The gpui key map is replaced wholesale (its
	/// `bind_keys` only appends) and the menu labels re-read the effective
	/// keys, so the change is live immediately — no restart.
	pub(crate) fn rebind_keys(&mut self, cx: &mut Context<Self>) {
		cx.clear_key_bindings();
		cx.bind_keys(crate::actions::key_bindings());
		self.rebuild_menu_bar(cx);
		cx.notify();
	}

	/// Re-evaluates the display color policy and retags every window after
	/// the color-management preference changed (the single-mapping rule must
	/// hold at runtime, not just at startup). Drops the cached display
	/// processors, then re-declares the policy on each open window.
	pub(crate) fn reapply_display_color(&mut self, cx: &mut Context<Self>) {
		crate::oakui::displaycolor::invalidate();
		for handle in cx.windows() {
			let _ = cx.update_window(handle, |_root, window, _app| {
				crate::oakui::displaycolor::apply_to_window(window);
			});
		}
		cx.notify();
	}

	/// Opens the action search dialog (Help > Search Actions…, the `/` key).
	/// Enter / double-click in the dialog executes the action through the same
	/// [`Self::dispatch_action_id`] path the menu clicks take, so the behavior
	/// can never diverge from a menu click.
	///
	/// The actual modal build is deferred to the end of the current app update:
	/// keyboard shortcuts and menu clicks dispatch *inside* a window update,
	/// and `spawn_modal`'s nested `update_window` would fail (and silently
	/// drop the dialog) there. `defer` runs after the window is back in the
	/// app's map, so the modal opens one tick later.
	pub fn open_action_search(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		let weak = cx.weak_entity();
		cx.defer(move |app| {
			if let Some(this) = weak.upgrade() {
				this.update(app, |this, cx| this.open_action_search_now(cx));
			}
		});
	}

	/// The deferred half of [`Self::open_action_search`]: builds the modal on
	/// the main window, subscribes to its execute events and focuses the search
	/// field.
	pub(crate) fn open_action_search_now(&mut self, cx: &mut Context<Self>) {
		self.spawn_modal(cx, |window, app| {
			let content = app.new(|cx| crate::dialogs::ActionSearchContent::new(window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::ACTION_SEARCH,
					ModalOptions::new(crate::i18n::tr("menu.help.action_search"), px(640.0))
						// Launcher-style: a backdrop click dismisses (same as
						// Escape and the title-row ✕).
						.with_dismiss_on_mask(true),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ActionSearch { modal, content }
		});
		if let ModalState::ActionSearch { content, .. } = &self.modal {
			let content = content.clone();
			cx.subscribe(
				&content,
				|this, _content, event: &crate::dialogs::ActionSearchEvent, cx| {
					let crate::dialogs::ActionSearchEvent::Execute(action) = event;
					this.close_modal(cx);
					this.dispatch_action_id(*action, cx);
				},
			)
			.detach();
			// Keyboard-first: focus the search field as soon as the dialog is
			// up (the shell's action dispatch already suppresses the global
			// key map while the modal is open).
			if let Some(handle) = cx.windows().first() {
				let content = content.clone();
				let _ = cx.update_window(*handle, |_root, window, app| {
					let focus = content.read(app).search_focus(app);
					window.focus(&focus, app);
				});
			}
		}
	}

	/// Opens the about dialog (Help > About Oak…): the app name, version,
	/// license line and fork notice. No engine interaction.
	pub(crate) fn open_about(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		self.spawn_modal(cx, |window, app| {
			let content = app.new(|_cx| crate::dialogs::AboutContent::new());
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::ABOUT,
					ModalOptions::new(crate::i18n::tr("about.title"), px(480.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						// Launcher-style dismissal is natural here too.
						.with_dismiss_on_mask(true),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::About { modal }
		});
	}

	/// Starts the startup update check unless the preferences disable it
	/// (`CheckForUpdates`): the blocking HTTPS fetch runs on the background
	/// executor and the prompt opens from its completion, so the first frame
	/// never waits on the network. Failures are logged and dropped.
	pub(crate) fn start_update_check(&mut self, cx: &mut Context<Self>) {
		if !crate::update::check_enabled() {
			return;
		}
		let transport = self.update_transport.clone();
		let executor = cx.background_executor().clone();
		cx.spawn(async move |this, cx| {
			let result = executor
				.spawn(async move { crate::update::fetch_latest(transport.as_ref()) })
				.await;
			let _ = this.update(cx, |this, cx| this.on_update_checked(result, cx));
		})
		.detach();
	}

	/// Apply the outcome of one update check, possibly opening the
	/// "update available" prompt.
	///
	/// `result` is what [`Self::start_update_check`]'s background fetch
	/// produced: `Ok(release)` with the newest release, or `Err(message)`
	/// from the transport/parse. A release that
	/// [`crate::update::should_notify`] considers newer than the running
	/// version is logged (`[update] <version> is available (running …)`) and
	/// handed to [`Self::open_update_available`], which either shows the
	/// dialog or stashes it behind the current modal. An equal/older release
	/// is silent, and a check failure only prints `[update] check failed: …`
	/// — no UI, no retry, so a machine that is offline at startup just sees
	/// nothing. `cx` is the entity context, used to spawn the dialog.
	pub(crate) fn on_update_checked(
		&mut self,
		result: Result<crate::update::LatestRelease, String>,
		cx: &mut Context<Self>,
	) {
		match result {
			Ok(release) if crate::update::should_notify(&release) => {
				println!(
					"[update] {} is available (running {})",
					release.version,
					crate::update::current_version()
				);
				self.open_update_available(release, cx);
			}
			Ok(_) => {}
			Err(error) => println!("[update] check failed: {error}"),
		}
	}

	/// Opens the update-available prompt; when another modal is up (the
	/// project manager on a fresh start) the release is stashed and shown by
	/// [`Self::close_modal`] once the modal layer frees up.
	pub(crate) fn open_update_available(
		&mut self,
		release: crate::update::LatestRelease,
		cx: &mut Context<Self>,
	) {
		if !matches!(self.modal, ModalState::None) {
			self.pending_update = Some(release);
			return;
		}
		let version = release.version.clone();
		let notes = release.notes.clone();
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|_cx| crate::dialogs::UpdateDialogContent::new(&version, &notes));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::UPDATE_AVAILABLE,
					ModalOptions::new(crate::i18n::tr("update.title"), px(520.0))
						.with_button(DialogButton::primary(crate::i18n::tr("update.download")))
						.with_button(DialogButton::new(
							crate::i18n::tr("update.later"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content)
			});
			ModalState::UpdateAvailable { modal }
		});
	}

	/// Test seam: replaces the update transport so tests can script the
	/// endpoint response without touching the network.
	#[cfg(test)]
	pub(crate) fn set_update_transport(
		&mut self,
		transport: Arc<dyn crate::update::UpdateTransport>,
	) {
		self.update_transport = transport;
	}
}

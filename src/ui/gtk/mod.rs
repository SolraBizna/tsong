use crate::*;
use gtk::{
    prelude::*,
    Align,
    Allocation,
    Application,
    ApplicationWindow,
    BoxBuilder,
    Button, ButtonBuilder, ButtonBoxBuilder, ButtonBoxStyle,
    CellRendererText,
    Container,
    Entry, EntryBuilder,
    Grid, GridBuilder,
    IconLookupFlags, IconTheme,
    Image,
    Label, LabelBuilder,
    ListStore,
    Orientation,
    Overlay,
    PolicyType,
    ScrolledWindowBuilder,
    SeparatorBuilder,
    SortType,
    Spinner, SpinnerBuilder,
    StyleContext,
    ToggleButton, ToggleButtonBuilder,
    ToolButton, ToolButtonBuilder,
    TreeIter, TreePath, TreeStore, TreeRowReference,
    TreeView, TreeViewBuilder, TreeViewColumn,
    VolumeButton, VolumeButtonBuilder,
    Widget,
};
use gdk::{
    Geometry,
    Gravity,
    Screen,
    WindowHints,
};
use glib::{
    types::Type,
    source::timeout_add_local,
    Value,
};
use gio::prelude::*;
use std::{
    cell::RefCell,
    rc::{Rc,Weak},
};

// TODO: this should be fluent...
const PLAYLIST_CODE_TOOLTIP: &str =
    "Enter playlist code here, e.g.:\n\
     \n\
     album:contains \"Moonlight\" or artist:starts_with \"The Answer\"\n\
     \n\
     Leave empty to include only manually added songs.";

/// Fallback labels for missing icons.
mod fallback {
    pub const ROLLUP: &str = "\u{1F783}\u{FE0E}";
    pub const ROLLDOWN: &str = "\u{1F781}\u{FE0E}";
    pub const SETTINGS: &str = "\u{2699}\u{FE0E}";
    pub const SHUFFLE: &str = "\u{1F500}\u{FE0E}";
    pub const LOOP: &str = "\u{1F501}\u{FE0E}";
    pub const LOOP_ONE: &str = "\u{1F502}\u{FE0E}";
    pub const PREV: &str = "\u{1F844}\u{FE0E}";
    pub const NEXT: &str = "\u{1F846}\u{FE0E}";
    // These don't look good...
    pub const PLAY: &str = "\u{23F5}\u{FE0E}";
    pub const PAUSE: &str = "\u{23F8}\u{FE0E}";
    // But this goes weird and off-center from the pause glyph...
    //pub const PLAY: &str = "\u{25B8}\u{FE0E}";
}

struct Controller {
    active_playlist: Option<PlaylistRef>,
    control_box: gtk::Box,
    delete_playlist_button: ToolButton,
    last_built_playlist: Option<PlaylistRef>,
    new_playlist_button: ToolButton,
    next_button: Button,
    osd: Label,
    play_button: Button,
    playlist_code: Entry,
    playlist_model: Option<ListStore>,
    playlist_name_cell: CellRendererText,
    playlist_name_column: TreeViewColumn,
    playlist_stats: Label,
    playlist_view: TreeView,
    playlists_model: TreeStore,
    playlists_view: TreeView,
    playmode_button: ToggleButton,
    prev_button: Button,
    rollup_button: Button,
    rollup_grid: Grid,
    settings_button: Button,
    shuffle_button: ToggleButton,
    volume_button: VolumeButton,
    window: ApplicationWindow,
    playlist_generation: GenerationValue,
    scan_spinner: Spinner,
    prev_icon: Option<Image>,
    play_icon: Option<Image>,
    pause_icon: Option<Image>,
    next_icon: Option<Image>,
    rollup_icon: Option<Image>,
    rolldown_icon: Option<Image>,
    shuffle_icon: Option<Image>,
    loop_icon: Option<Image>,
    loop_one_icon: Option<Image>,
    scan_thread: ScanThread,
    rolled_down_height: i32,
    me: Option<Weak<RefCell<Controller>>>,
}

fn set_image(button: &Button, icon: &Option<Image>, fallback: &str) {
    button.set_image(icon.as_ref());
    match icon {
        Some(_) => button.set_label(""),
        None => button.set_label(fallback),
    }
}

impl Controller {
    pub fn new(rollup_button: Button, settings_button: Button,
               prev_button: Button, next_button: Button,
               shuffle_button: ToggleButton, playmode_button: ToggleButton,
               play_button: Button, volume_button: VolumeButton,
               rollup_grid: Grid, new_playlist_button: ToolButton,
               delete_playlist_button: ToolButton,
               playlists_view: TreeView, playlist_view: TreeView,
               playlist_code: Entry, playlist_stats: Label, osd: Label,
               scan_spinner: Spinner, control_box: gtk::Box,
               window: ApplicationWindow)
        -> Rc<RefCell<Controller>> {
        let mut scan_thread = ScanThread::new();
        scan_thread.rescan(prefs::get_music_paths())
            .expect("Couldn't start the initial music scan!");
        let playlist_model = None;
        let (playlists_model, _) = build_playlists_model(&[]);
        let playlist_name_column = TreeViewColumn::new();
        let playlist_name_cell = CellRendererText::new();
        playlist_name_cell.set_property("editable", &true)
            .expect("couldn't make playlist name cell editable");
        playlist_name_column.pack_start(&playlist_name_cell, true);
        playlist_name_column.add_attribute(&playlist_name_cell, "text", 1);
        // I'd love to do this in CSS...
        // (ignore errors because this is not critical to functionality)
        // (fun fact! if this is an f32 instead of an f64, it breaks!)
        let _ = playlist_name_cell.set_property("scale", &0.80);
        let nu = Rc::new(RefCell::new(Controller {
            rollup_button, settings_button, prev_button, next_button,
            shuffle_button, playmode_button, play_button, volume_button,
            playlists_view, playlist_view, playlist_code,
            playlists_model, playlist_model, playlist_stats, osd,
            scan_spinner, scan_thread, rollup_grid, control_box,
            new_playlist_button, delete_playlist_button,
            playlist_name_column, playlist_name_cell, window,
            prev_icon: None, next_icon: None,
            play_icon: None, pause_icon: None,
            rollup_icon: None, rolldown_icon: None,
            loop_icon: None, loop_one_icon: None, shuffle_icon: None,
            active_playlist: None, playlist_generation: Default::default(),
            last_built_playlist: None, me: None,
            rolled_down_height: 400,
        }));
        // Throughout this function, we make use of a hack.
        // Each signal that depends on the Controller starts with an attempt to
        // mutably borrow the controller. If said attempt fails, that means
        // that the signal was raised by other code called from within the
        // controller, so we ignore the signal.
        let mut this = nu.borrow_mut();
        this.me = Some(Rc::downgrade(&nu));
        this.delete_playlist_button
            .set_sensitive(this.delete_playlist_button_should_be_sensitive());
        this.reload_icons();
        this.settings_button.set_sensitive(false);
        this.playlists_view.append_column(&this.playlist_name_column);
        this.volume_button.connect_value_changed(|_, value| {
            prefs::set_volume((value * 100.0).floor() as i32);
        });
        let controller = nu.clone();
        this.playlist_code.connect_property_text_notify(move |_| {
            let _ = controller.try_borrow()
                .map(|x| x.update_playlist_code());
        });
        this.prev_button.connect_clicked(|_| {
            playback::send_command(PlaybackCommand::Prev)
        });
        this.next_button.connect_clicked(|_| {
            playback::send_command(PlaybackCommand::Next)
        });
        let controller = nu.clone();
        this.rollup_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_rollup());
        });
        let controller = nu.clone();
        this.play_button.connect_clicked(move |_| {
            let _ = controller.try_borrow()
                .map(|x| x.clicked_play());
        });
        this.playlists_view.set_model(Some(&this.playlists_model));
        let controller = nu.clone();
        this.playlist_name_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow()
                .map(|x| x.edited_playlist_name(wo, nu));
        });
        let controller = nu.clone();
        this.playlists_view.connect_cursor_changed(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.playlists_cursor_changed());
        });
        let controller = nu.clone();
        this.playlist_view.connect_row_activated(move |_, wo, _| {
            let _ = controller.try_borrow()
                .map(|x| x.playlist_row_activated(wo));
        });
        let controller = nu.clone();
        this.shuffle_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_shuffle());
        });
        let controller = nu.clone();
        this.playmode_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_playmode());
        });
        let controller = nu.clone();
        this.new_playlist_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_new_playlist());
        });
        let controller = nu.clone();
        this.delete_playlist_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_playlist());
        });
        let controller = nu.clone();
        this.window.connect_size_allocate(move |_, allocation| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.main_window_resized(allocation));
        });
        this.activate_playlist_by_path(&TreePath::new_first());
        this.periodic();
        drop(this);
        nu
    }
    fn rebuild_playlist_view(&mut self) {
        let song_to_select = match self.last_built_playlist.as_ref() {
            Some(playlist) if Some(playlist) == self.active_playlist.as_ref()
                => {
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x, 0))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            },
            _ => None,
        };
        self.last_built_playlist = self.active_playlist.clone();
        // destroy existing columns
        // TODO: figure out why the sort indicator disappears
        while self.playlist_view.get_n_columns() > 0 {
            let n = (self.playlist_view.get_n_columns() - 1) as i32;
            let column = self.playlist_view.get_column(n).unwrap();
            column.set_sort_indicator(false);
            self.playlist_view.remove_column(&column);
        }
        let tvc = TreeViewColumn::new();
        let cell = CellRendererText::new();
        cell.set_alignment(1.0, 0.5);
        tvc.set_title("#");
        tvc.set_clickable(false);
        tvc.set_fixed_width(50);
        tvc.set_sort_indicator(false);
        tvc.pack_start(&cell, true);
        tvc.set_alignment(1.0);
        tvc.add_attribute(&cell, "text", 1);
        self.playlist_view.append_column(&tvc);
        let playlist_ref = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => {
                self.playlist_model = None;
                self.playlist_view.set_model::<ListStore>(None);
                self.playlist_generation.destroy();
                self.shuffle_button.set_sensitive(false);
                self.shuffle_button.set_active(false);
                self.update_playmode_button();
                return
            },
        };
        let playlist = playlist_ref.maybe_refreshed();
        self.shuffle_button.set_sensitive(true);
        self.shuffle_button.set_active(playlist.is_shuffled());
        self.playlist_generation = playlist.get_playlist_generation();
        let mut types = Vec::with_capacity(playlist.get_columns().len() + 2);
        types.push(SONG_ID_TYPE); // Song ID
        types.push(Type::U32); // Index in playlist
        for _ in playlist.get_columns() {
            types.push(Type::String); // Each metadata column...
        }
        let first_sort_by = playlist.get_sort_order().get(0);
        let playlist_model = ListStore::new(&types[..]);
        let mut column_index: u32 = 2;
        for column in playlist.get_columns() {
            let tvc = TreeViewColumn::new();
            let cell = CellRendererText::new();
            tvc.set_title(&make_column_heading(&column.tag));
            tvc.set_clickable(true);
            tvc.set_resizable(true);
            tvc.set_fixed_width(column.width as i32);
            {
                let controller = Weak::upgrade(self.me.as_ref().unwrap())
                    .unwrap();
                let playlist_ref = playlist_ref.clone();
                let column_tag = column.tag.clone();
                tvc.connect_clicked(move |_| {
                    playlist_ref.write().unwrap().touched_heading(&column_tag);
                    controller.borrow_mut().rebuild_playlist_view();
                });
            }
            {
                let playlist_ref = playlist_ref.clone();
                let column_tag = column.tag.clone();
                tvc.connect_property_width_notify(move |tvc| {
                    playlist_ref.write().unwrap().resize_column(&column_tag,
                                                                tvc.get_width()
                                                                as u32);
                });
            }
            match first_sort_by {
                Some((tag,descending)) if tag == &column.tag => {
                    tvc.set_sort_indicator(true);
                    tvc.set_sort_order(if *descending { SortType::Descending }
                                       else { SortType::Ascending });
                },
                _ => {
                    tvc.set_sort_indicator(false);
                },
            };
            tvc.pack_start(&cell, true);
            if column.tag == "duration" || column.tag == "year"
            || column.tag.ends_with("_number") {
                cell.set_alignment(1.0, 0.5);
                tvc.set_alignment(1.0);
            }
            tvc.add_attribute(&cell, "text", column_index as i32);
            // TODO: i18n this
            column_index += 1;
            self.playlist_view.append_column(&tvc);
        }
        let tvc = TreeViewColumn::new();
        tvc.set_title(fallback::SETTINGS);
        tvc.set_clickable(true);
        self.playlist_view.append_column(&tvc);
        let mut song_index = 1;
        let mut total_duration = 0u32;
        let mut place_to_put_cursor = None;
        for song in playlist.get_songs() {
            let new_row = playlist_model.append();
            if Some(song) == song_to_select.as_ref() {
                place_to_put_cursor = playlist_model.get_path(&new_row);
            }
            let song = song.read().unwrap();
            playlist_model.set_value(&new_row, 0,
                                     &song_id_to_value(song.get_id()));
            playlist_model.set_value(&new_row, 1, &song_index.to_value());
            song_index += 1;
            total_duration = total_duration.saturating_add(song.get_duration());
            let metadata = song.get_metadata();
            let mut column_index: u32 = 2;
            for column in playlist.get_columns() {
                let s = if column.tag == "duration" {
                    pretty_duration(song.get_duration()).to_value()
                }
                else {
                    metadata.get(&column.tag).map(String::as_str)
                        .and_then(|x| if x.len() == 0 { None } else { Some(x)})
                        .to_value()
                };
                playlist_model.set_value(&new_row, column_index, &s);
                column_index += 1;
            }
        }
        self.playlist_view.set_model(Some(&playlist_model));
        self.playlist_model = Some(playlist_model);
        if let Some(place) = place_to_put_cursor {
            self.playlist_view.set_cursor::<TreeViewColumn>
                (&place, None, false);
        }
        match song_index-1 {
            0 => self.playlist_stats.set_label("No songs in playlist"),
            x => {
                let t = format!("{} song{} in playlist, total time {}",
                                x, if x == 1 { "" } else { "s" },
                                if total_duration == u32::MAX {
                                    "really really long".to_owned()
                                }
                                else {
                                    pretty_duration(total_duration)
                                });
                self.playlist_stats.set_label(&t);
            }
        }
        drop(playlist);
        self.update_playmode_button();
    }
    fn reload_icons(&mut self) {
        // TODO: reload icons when theme is changed
        // note: figure this out later, for now, be sad that the stock icons
        // are inadequate
        self.prev_icon = get_icon("tsong-previous");
        self.next_icon = get_icon("tsong-next");
        self.play_icon = get_icon("tsong-play");
        self.pause_icon = get_icon("tsong-pause");
        self.rollup_icon = get_icon("tsong-rollup");
        self.rolldown_icon = get_icon("tsong-rolldown");
        self.shuffle_icon = get_icon("tsong-shuffle");
        self.loop_icon = get_icon("tsong-loop");
        self.loop_one_icon = get_icon("tsong-loop-one");
        set_image(&self.prev_button, &self.prev_icon, fallback::PREV);
        set_image(&self.next_button, &self.next_icon, fallback::NEXT);
        if playback::get_playback_status().is_playing() {
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
        }
        else {
            set_image(&self.play_button, &self.play_icon, fallback::PLAY);
        }
        if self.rollup_grid.get_visible() {
            set_image(&self.rollup_button, &self.rollup_icon,
                      fallback::ROLLUP);
        }
        else {
            set_image(&self.rollup_button, &self.rolldown_icon,
                      fallback::ROLLDOWN);
        }
        set_image(self.shuffle_button.upcast_ref(), &self.shuffle_icon,
                  fallback::SHUFFLE);
        set_image(self.playmode_button.upcast_ref(), &self.loop_icon,
                  fallback::LOOP);
    }
    fn activate_playlist_by_path(&mut self, wo: &TreePath) {
        let id = match self.playlists_model.get_iter(wo)
            .map(|x| self.playlists_model.get_value(&x, 0))
            .and_then(value_to_playlist_id)
        {
            Some(id) => id,
            None => return,
        };
        let playlist = match playlist::get_playlist_by_id(id) {
            Some(playlist) => playlist,
            None => {
                eprintln!("Warning: Tried to activate playlist ID {} by path \
                           {}, but it doesn't exist!", id, wo);
                return
            },
        };
        if Some(&playlist) == self.active_playlist.as_ref() {
            return
        }
        self.active_playlist = Some(playlist.clone());
        self.playlist_generation.destroy();
        self.playlist_code.set_text(playlist.read().unwrap().get_rule_code());
        let style_context = self.playlist_code.get_style_context();
        style_context.remove_class("error");
        self.rebuild_playlist_view();
        let selection = self.playlists_view.get_selection();
        selection.select_path(wo);
    }
    fn periodic(&mut self) {
        self.update_view();
        self.update_scan_status();
        self.maybe_rebuild_playlist();
        let timeout_ms = if playback::get_playback_status().is_playing() { 100 }
        else { 1000 };
        let controller = match self.me.as_ref().and_then(Weak::upgrade) {
            None => return,
            Some(x) => x,
        };
        timeout_add_local(timeout_ms, move || {
            controller.borrow_mut().periodic();
            Continue(false)
        });
    }
    fn update_view(&mut self) {
        let (status, active_song) = playback::get_status_and_active_song();
        if status.is_playing() {
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
        }
        else {
            set_image(&self.play_button, &self.play_icon, fallback::PLAY);
        }
        match active_song {
            None => {
                self.osd.set_label("");
            },
            Some((song, time)) => {
                let song = song.read().unwrap();
                let metadata = song.get_metadata();
                self.osd.set_label(&format!("{} - {}\n{} / {}",
                                                metadata.get("title").map(String::as_str).unwrap_or("Unknown Title"),
                                                metadata.get("artist").map(String::as_str).unwrap_or("Unknown Artist"),
                                                pretty_duration(time.floor() as u32),
                                                pretty_duration(song.get_duration())));
            },
        }
    }
    fn update_scan_status(&mut self) {
        let scan_in_progess = match self.scan_thread.get_result_nonblocking() {
            Err(x) => {
                // TODO: display this error
                eprintln!("Warning: Scan thread crashed! {:?}", x);
                false
            },
            Ok((x, None)) => !x,
            Ok((true, _)) => unreachable!(),
            Ok((false, Some(Ok(_)))) => {
                // (We would try updating the playlist here, except that that
                // will already have happened, because `update_view()` is
                // called before us)
                true
            },
            Ok((false, Some(Err(x)))) => {
                // TODO: display this error
                eprintln!("Warning: Error during scan! {:?}", x);
                true
            },
        };
        if scan_in_progess {
            self.scan_spinner.start();
        }
        else {
            self.scan_spinner.stop();
        }
    }
    fn maybe_rebuild_playlist(&mut self) {
        let playlist = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = match playlist.sheepishly_maybe_refreshed() {
            Some(x) => x,
            _ => return,
        };
        if playlist.get_playlist_generation() == self.playlist_generation {
            return
        }
        drop(playlist);
        self.rebuild_playlist_view();
    }
    fn update_playlist_code(&self) {
        let value = self.playlist_code.get_text();
        if let Some(playlist) = self.active_playlist.as_ref() {
            let mut playlist = playlist.write().unwrap();
            let style_context = self.playlist_code.get_style_context();
            match playlist.set_rule_code(value.into()) {
                Err(x) => {
                    style_context.add_class("error");
                    self.playlist_code.set_tooltip_text(Some(&x));
                },
                Ok(_) => {
                    style_context.remove_class("error");
                    self.playlist_code
                        .set_tooltip_text(Some(PLAYLIST_CODE_TOOLTIP));
                }
            }
        }
    }
    fn clicked_play(&self) {
        let status = playback::get_playback_status();
        if status.is_playing() {
            playback::send_command(PlaybackCommand::Pause);
            set_image(&self.play_button, &self.play_icon, fallback::PLAY);
        }
        else {
            let song_to_play = if status == PlaybackStatus::Stopped {
                playback::set_future_playlist(self.active_playlist.clone());
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x, 0))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            } else { None };
            playback::send_command(PlaybackCommand::Play(song_to_play));
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
        }
    }
    fn edited_playlist_name(&self, wo: TreePath, nu: &str) -> Option<()> {
        let iter = self.playlists_model.get_iter(&wo)?;
        let value = self.playlists_model.get_value(&iter, 0);
        let playlist = value_to_playlist_id(value)
            .and_then(playlist::get_playlist_by_id)?;
        self.playlists_model.set_value(&iter, 1, &Value::from(nu));
        playlist.write().unwrap().set_name(nu.to_owned());
        None
    }
    fn playlists_cursor_changed(&mut self) -> Option<()> {
        let wo = self.playlists_view.get_cursor().0?;
        self.activate_playlist_by_path(&wo);
        None
    }
    fn playlist_row_activated(&self, wo: &TreePath) -> Option<()> {
        let playlist_model = self.playlist_model.as_ref()?;
        let song = playlist_model.get_iter(wo)
            .map(|x| playlist_model.get_value(&x, 0))
            .and_then(value_to_song_id)
            .and_then(logical::get_song_by_song_id);
        if let Some(song) = song {
            playback::set_future_playlist(self.active_playlist.clone());
            playback::send_command(PlaybackCommand::Play(Some(song)));
        }
        None
    }
    fn clicked_shuffle(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        self.shuffle_button.set_active(playlist.write().unwrap()
                                       .toggle_shuffle());
        self.rebuild_playlist_view();
        None
    }
    fn clicked_playmode(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        playlist.write().unwrap().bump_playmode();
        self.update_playmode_button();
        self.rebuild_playlist_view();
        None
    }
    fn update_playmode_button(&mut self) -> Option<()> {
        match self.active_playlist.as_ref() {
            None => {
                self.playmode_button.set_sensitive(false);
                self.playmode_button.set_active(false);
                set_image(self.playmode_button.upcast_ref(),
                          &self.loop_icon,
                          fallback::LOOP);
            },
            Some(playlist) => {
                self.playmode_button.set_sensitive(true);
                let playmode = playlist.read().unwrap().get_playmode();
                if playmode == Playmode::LoopOne {
                    set_image(self.playmode_button.upcast_ref(),
                              &self.loop_one_icon,
                              fallback::LOOP_ONE);
                }
                else {
                    set_image(self.playmode_button.upcast_ref(),
                              &self.loop_icon,
                              fallback::LOOP);
                }
                self.playmode_button.set_active(playmode != Playmode::End);
            }
        }
        None
    }
    fn clicked_new_playlist(&mut self) -> Option<()> {
        let playlist = match playlist::create_new_playlist() {
            Ok(x) => x,
            Err(x) => {
                // TODO: display error better
                eprintln!("Unable to create playlist: {:?}", x);
                return None
            }
        };
        let playlist = playlist.read().unwrap();
        let id = playlist.get_id();
        let playlists_model = &self.playlists_model;
        let wo = 
            playlists_model.insert_with_values(None, None, &[0, 1],
                                               &[&playlist_id_to_value(id),
                                                 &playlist.get_name()]);
        drop(playlist);
        match playlists_model.get_path(&wo) {
            Some(path) => {
                self.activate_playlist_by_path(&path);
                self.playlists_view
                    .set_cursor_on_cell(&path,
                                        Some(&self.playlist_name_column),
                                        Some(&self.playlist_name_cell),
                                        true);
            },
            _ => (),
        }
        None
    }
    fn delete_playlist_button_should_be_sensitive(&self) -> bool {
        // TODO: better safety logic, and use TreeSelection::
        // count_selected_rows()
        playlist::get_top_level_playlists().len() > 1
    }
    fn clicked_delete_playlist(&mut self) -> Option<()> {
        if !self.delete_playlist_button_should_be_sensitive() { return None }
        let _playlist = self.active_playlist.as_ref()?;
        // TODO: If deleting one or more playlists with children, warn before
        // proceeding
        let selection = self.playlists_view.get_selection();
        let (wo_list, model) = selection.get_selected_rows();
        let wo_list: Vec<TreeRowReference>
            = wo_list.into_iter()
            .filter_map(|x| TreeRowReference::new(&model, &x))
            .collect();
        for wo in wo_list.iter() {
            let playlist = match wo.get_path()
                .and_then(|x| self.playlists_model.get_iter(&x))
                .map(|x| self.playlists_model.get_value(&x, 0))
                .and_then(value_to_playlist_id)
                .and_then(playlist::get_playlist_by_id)
            {
                Some(x) => x,
                None => continue,
            };
            if Some(&playlist) == self.active_playlist.as_ref() {
                self.active_playlist = None;
            }
            playlist::delete_playlist(playlist);
        }
        let (neu_model, _) = build_playlists_model(&[]);
        self.playlists_model = neu_model;
        self.playlists_view.set_model(Some(&self.playlists_model));
        if self.active_playlist.is_none() {
            self.activate_playlist_by_path(&TreePath::new_first());
        }
        self.delete_playlist_button
            .set_sensitive(self.delete_playlist_button_should_be_sensitive());
        None
    }
    fn clicked_rollup(&mut self) {
        let mut geom = Geometry {
            min_width: -1, max_width: i32::MAX,
            min_height: -1, max_height: i32::MAX,
            base_width: -1, base_height: -1,
            width_inc: -1, height_inc: -1,
            min_aspect: -1.0, max_aspect: -1.0,
            win_gravity: Gravity::NorthWest,
        };
        let geom_mask = WindowHints::MAX_SIZE;
        if self.rollup_grid.get_visible() {
            geom.max_height = self.control_box.get_allocated_height();
            self.rolled_down_height = self.window.get_allocated_height();
            self.rollup_grid.hide();
            set_image(&self.rollup_button, &self.rolldown_icon,
                      fallback::ROLLDOWN);
            self.window.set_geometry_hints(Some(&self.window),
                                           Some(&geom), geom_mask);
        }
        else {
            self.rollup_grid.show();
            set_image(&self.rollup_button, &self.rollup_icon,
                      fallback::ROLLUP);
            self.window.set_geometry_hints(Some(&self.window),
                                           Some(&geom), geom_mask);
            self.window.resize(self.window.get_allocated_width(),
                               self.rolled_down_height);
        }
    }
    fn main_window_resized(&mut self, allocation: &Allocation) {
        if !self.rollup_grid.is_visible() {
            if allocation.height >= self.control_box.get_allocated_height()*3/2
            {
                self.clicked_rollup();
                self.rollup_button.set_sensitive(false);
            }
        }
    }
}

fn add_klasoj<W>(widget: &W, klasoj: &[&str])
where W: IsA<Widget> {
    let style_context = widget.get_style_context();
    for klaso in klasoj {
        style_context.add_class(klaso);
    }
}

fn control_button_add<T, W>(control_box: &T, button: &W, klasoj: &[&str])
where T: IsA<Container>, W: IsA<Widget> {
    add_klasoj(button, klasoj);
    let nu_box = BoxBuilder::new().orientation(Orientation::Vertical)
        .valign(Align::Center).build();
    nu_box.pack_start(button, false, false, 0);
    control_box.add(&nu_box);
}

// O_O
fn get_icon(wat: &str) -> Option<Image> {
    let icon_theme = IconTheme::get_default()?;
    let icon = icon_theme.load_icon(wat, 24, IconLookupFlags::empty()).ok()?;
    match icon {
        None => None,
        Some(icon) => Some(Image::from_pixbuf(Some(&icon))),
    }
}

pub fn go() {
    let application = Application::new(
        Some("name.bizna.tsong"),
        Default::default(),
    ).expect("failed to initialize the GTK application (!?!)");
    application.connect_activate(|application| {
        let window = ApplicationWindow::new(application);
        window.set_title("Tsong");
        window.set_default_size(640, 460);
        let provider = gtk::CssProvider::new();
        provider.load_from_data(include_bytes!("css.css")).unwrap();
        StyleContext::add_provider_for_screen(&Screen::get_default().unwrap(),
                                              &provider, 750);
        let outer_box = BoxBuilder::new().orientation(Orientation::Vertical)
            .build();
        window.add(&outer_box);
        // The outer box is divided into two things.
        // One, a fixed-height row that contains the playback controls:
        let control_box = BoxBuilder::new()
            .name("controls")
            .spacing(4).hexpand(true).build();
        outer_box.add(&control_box);
        // Two, taking up the rest of the window, a variable-height box that
        // contains the playlist controls:
        let rollup_grid = GridBuilder::new()
            .expand(true).build();
        // So, the playback controls...
        // Button to bring up the settings window:
        let settings_button = ButtonBuilder::new()
            .name("settings").label(fallback::SETTINGS).build();
        control_button_add(&control_box, &settings_button, &["popup"]);
        // Button to go back to the previous song in the playlist:
        let prev_button = ButtonBuilder::new()
            .name("prev").build();
        control_button_add(&control_box, &prev_button, &["circular"]);
        // Explicit play/pause button
        let play_button = ButtonBuilder::new()
            .name("playpause").build();
        control_button_add(&control_box, &play_button, &["circular"]);
        // Osd widget!
        let osd = LabelBuilder::new()
            .name("osd")
            .hexpand(true).build();
        control_box.add(&osd);
        // Button to go to the next song in the playlist:
        let next_button = ButtonBuilder::new()
            .name("next").build();
        control_button_add(&control_box, &next_button, &["circular"]);
        // Volume slider:
        let volume_slider = VolumeButtonBuilder::new()
            .name("volume")
            .margin_top(7).margin_bottom(7)
            .value(prefs::get_volume() as f64 / 100.0)
            .build();
        control_box.add(&volume_slider);
        // Button to "roll up" the playlist box:
        let rollup_button = ButtonBuilder::new()
            .name("rollup").build();
        control_button_add(&control_box, &rollup_button, &["toggle"]);
        // That's the end of the playback controls.
        // Now, the playlists!
        let separator = SeparatorBuilder::new()
            .orientation(Orientation::Vertical).build();
        rollup_grid.attach(&separator, 1, 0, 1, 2);
        let playlists_box = BoxBuilder::new()
            .orientation(Orientation::Vertical).build();
        let playlists_window = ScrolledWindowBuilder::new()
            .name("playlists")
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let playlists_view = TreeViewBuilder::new()
            .headers_visible(false).reorderable(true).build();
        playlists_window.add(&playlists_view);
        playlists_box.add(&playlists_window);
        rollup_grid.attach(&playlists_box, 0, 0, 1, 1);
        let playlist_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_playlist_button
            = ToolButtonBuilder::new().icon_name("list-remove").build();
        playlist_button_box.add(&delete_playlist_button);
        let new_playlist_button
            = ToolButtonBuilder::new().icon_name("list-add").build();
        playlist_button_box.add(&new_playlist_button);
        rollup_grid.attach(&playlist_button_box, 0, 1, 1, 1);
        // and the play...list
        let playlist_itself_box = BoxBuilder::new()
            .name("playlist").orientation(Orientation::Vertical).build();
        let playlist_meta_box = ButtonBoxBuilder::new()
            .name("meta").layout_style(ButtonBoxStyle::Expand)
            .homogeneous(false)
            .orientation(Orientation::Horizontal).build();
        // make the right edge merge with the window edge :)
        playlist_meta_box.pack_end(&BoxBuilder::new().build(), false, false, 0);
        // Button to change shuffle mode:
        let shuffle_button = ToggleButtonBuilder::new()
            .name("shuffle").build();
        playlist_meta_box.pack_start(&shuffle_button, false, false, 0);
        // Button to change loop mode:
        let playmode_button = ToggleButtonBuilder::new()
            .name("playmode").build();
        playlist_meta_box.pack_end(&playmode_button, false, false, 0);
        // The playlist code:
        let playlist_code = EntryBuilder::new().hexpand(true)
            .placeholder_text("Manually added songs only")
            .tooltip_text(PLAYLIST_CODE_TOOLTIP)
            .build();
        playlist_meta_box.pack_start(&playlist_code, true, true, 0);
        // TODO: make this a monospace font?
        playlist_itself_box.add(&playlist_meta_box);
        // The playlist itself:
        let playlist_window = ScrolledWindowBuilder::new()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true).hexpand(true)
            .build();
        let playlist_view = TreeViewBuilder::new().expand(true)
            .headers_visible(true).build();
        playlist_window.add(&playlist_view);
        playlist_itself_box.add(&playlist_window);
        rollup_grid.attach(&playlist_itself_box, 2, 0, 1, 1);
        let bottom_overlay = Overlay::new();
        let playlist_stats = LabelBuilder::new()
            .name("playlist_stats").build();
        bottom_overlay.add(&playlist_stats);
        let scan_spinner = SpinnerBuilder::new().name("scan_spinner")
            .halign(Align::End).valign(Align::Center).build();
        bottom_overlay.add_overlay(&scan_spinner);
        rollup_grid.attach(&bottom_overlay, 2, 1, 1, 1);
        outer_box.add(&rollup_grid);
        // okay, show the window and away we go
        window.show_all();
        // Controller will hook itself in and keep track of its own lifetime
        let _ = Controller::new(rollup_button, settings_button, prev_button,
                                next_button, shuffle_button, playmode_button,
                                play_button, volume_slider, rollup_grid,
                                new_playlist_button, delete_playlist_button,
                                playlists_view, playlist_view,
                                playlist_code, playlist_stats, osd,
                                scan_spinner, control_box, window);
    });
    // don't parse a command line, apparently? the documentation is fuzzy on
    // what that would entail anyway...
    application.run(&[]);
}

fn pretty_duration(t: u32) -> String {
    if t >= 86400 {
        format!("{}:{:02}:{:02}:{:02}",
                t / 86400, (t / 3600) % 24, (t / 60) % 60, t % 60)
    }
    else if t >= 3600 {
        format!("{}:{:02}:{:02}",
                t / 3600, (t / 60) % 60, t % 60)
    }
    else {
        format!("{}:{:02}",
                t / 60, t % 60)
    }
}

/// Take a metadata tag name and return its human-readable name.
fn make_column_heading(orig: &str) -> String {
    let mut ret = Vec::with_capacity(orig.as_bytes().len());
    let mut spaced = true;
    for b in orig.as_bytes() {
        if *b == b'_' || *b == b' ' {
            if ret.len() > 0 {
                ret.push(b' ');
                spaced = true;
            }
        }
        else {
            if *b >= b'a' && *b <= b'z' && spaced {
                ret.push(b - 0x20);
            }
            else {
                ret.push(*b);
            }
            spaced = false;
        }
    }
    while ret.len() > 0 && ret[ret.len()-1] == b' ' {
        ret.pop();
    }
    // This unsafe code is safe because the original string was valid UTF-8,
    // and we only transformed it by removing ASCII space and underscore from
    // the beginning or end (safe) or transforming ASCII lowercase into ASCII
    // uppercase (safe) or transforming ASCII underscore into ASCII space
    // (safe).
    let ret = unsafe { String::from_utf8_unchecked(ret) };
    // TODO: pass the "Englishified" heading into internationalization when we
    // add that. This will not only make the above, highly-English-specific,
    // code functional, but will replace the following hack.
    if ret == "Duration" { "Time".to_owned() }
    else { ret }
}

const PLAYLIST_ID_TYPE: Type = Type::U64;
const SONG_ID_TYPE: Type = Type::U64;

fn playlist_id_to_value(id: PlaylistID) -> Value {
    id.as_inner().to_value()
}

fn value_to_playlist_id(id: Value) -> Option<PlaylistID> {
    id.get().ok().and_then(|x| x).map(PlaylistID::from_inner)
}

fn song_id_to_value(id: SongID) -> Value {
    id.as_inner().to_value()
}

fn value_to_song_id(id: Value) -> Option<SongID> {
    id.get().ok().and_then(|x| x).map(SongID::from_inner)
}

fn add_playlists_to_model(playlists_model: &TreeStore,
                          selected_playlists: &[PlaylistRef],
                          selection_paths: &mut Vec<TreePath>,
                          parent_iterator: Option<&TreeIter>,
                          children: &[PlaylistRef]) {
    for playlist_ref in children.iter() {
        let playlist = playlist_ref.read().unwrap();
        let id = playlist.get_id();
        let iter
            = playlists_model.insert_with_values(parent_iterator, None, &[0,1],
                                                 &[&playlist_id_to_value(id),
                                                   &playlist.get_name()]);
        if selected_playlists.contains(playlist_ref) {
            match playlists_model.get_path(&iter) {
                Some(x) => selection_paths.push(x),
                None => (),
            }
        }
        add_playlists_to_model(playlists_model,
                               selected_playlists,
                               selection_paths,
                               Some(&iter),
                               playlist.get_children());
    }
}

fn build_playlists_model(selected_playlists: &[PlaylistRef])
-> (TreeStore, Vec<TreePath>) {
    let playlists_model = TreeStore::new(&[PLAYLIST_ID_TYPE,Type::String]);
    let mut selection_paths = Vec::with_capacity(selected_playlists.len());
    add_playlists_to_model(&playlists_model, selected_playlists,
                           &mut selection_paths, None,
                           &playlist::get_top_level_playlists()[..]);
    (playlists_model, selection_paths)
}

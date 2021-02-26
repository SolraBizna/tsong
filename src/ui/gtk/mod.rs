use crate::*;
use gtk::{
    prelude::*,
    Align,
    Allocation,
    Application,
    ApplicationWindow,
    BoxBuilder,
    Button, ButtonBuilder, ButtonBoxBuilder, ButtonBoxStyle,
    ButtonsType,
    CellRendererText,
    Container,
    DialogFlags,
    Entry, EntryBuilder,
    Grid, GridBuilder,
    IconLookupFlags, IconTheme,
    Image,
    Label, LabelBuilder,
    ListStore,
    MessageDialog, MessageType,
    Orientation,
    Overlay,
    PolicyType,
    ResponseType,
    ScrolledWindowBuilder,
    SeparatorBuilder,
    Spinner, SpinnerBuilder,
    StateFlags,
    StyleContext,
    ToggleButton, ToggleButtonBuilder,
    ToolButton, ToolButtonBuilder,
    TreeIter, TreePath, TreeStore, TreeRowReference, TreeModelFlags,
    TreeView, TreeViewBuilder, TreeViewColumn,
    VolumeButton, VolumeButtonBuilder,
    Widget,
};
use gdk::{
    Geometry,
    Gravity,
    RGBA,
    Screen,
    WindowHints,
};
use glib::{
    types::Type,
    source::{SourceId, source_remove, timeout_add_local},
    Value,
};
use gio::prelude::*;
use mpris_player::{
    MprisPlayer,
    OrgMprisMediaPlayer2Player,
};
use std::{
    cell::RefCell,
    rc::{Rc,Weak},
    sync::Arc,
};

mod settings;
mod playlist_edit;

const INACTIVE_WEIGHT: u32 = 400; // normal weight
const ACTIVE_WEIGHT: u32 = 800; // bold

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

pub struct Controller {
    active_playlist: Option<PlaylistRef>,
    control_box: gtk::Box,
    delete_playlist_button: ToolButton,
    last_built_playlist: Option<PlaylistRef>,
    new_playlist_button: ToolButton,
    next_button: Button,
    osd: Label,
    play_button: Button,
    playlist_name: Entry,
    playlist_model: Option<ListStore>,
    playlist_name_cell: CellRendererText,
    playlist_name_column: TreeViewColumn,
    playlist_stats: Label,
    playlist_view: TreeView,
    playlists_model: TreeStore,
    playlists_view: TreeView,
    playmode_button: ToggleButton,
    playlist_edit_button: Button,
    prev_button: Button,
    rollup_button: Button,
    rollup_grid: Grid,
    settings_button: Button,
    shuffle_button: ToggleButton,
    volume_button: VolumeButton,
    window: ApplicationWindow,
    playlist_generation: GenerationValue,
    scan_spinner: Spinner,
    mpris_player: Arc<MprisPlayer>,
    mpris_time: i64,
    last_active_playlist: Option<(TreeIter,PlaylistRef)>,
    last_active_song: Option<(Option<TreeIter>,LogicalSongRef)>,
    prev_icon: Option<Image>,
    play_icon: Option<Image>,
    pause_icon: Option<Image>,
    next_icon: Option<Image>,
    rollup_icon: Option<Image>,
    rolldown_icon: Option<Image>,
    shuffle_icon: Option<Image>,
    loop_icon: Option<Image>,
    loop_one_icon: Option<Image>,
    settings_icon: Option<Image>,
    scan_thread: ScanThread,
    rolled_down_height: i32,
    settings_controller: Option<Rc<RefCell<settings::Controller>>>,
    playlist_edit_controller: Option<Rc<RefCell<playlist_edit::Controller>>>,
    periodic_timer: Option<SourceId>,
    volume_changed: bool,
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
    pub fn new(application: &Application) -> Rc<RefCell<Controller>> {
        if let Some(path) = std::env::vars().find_map(|(x,y)| {
            if x == "TSONG_ICON_PATH" { Some(y) } else { None }
        }) {
            let icon_theme = IconTheme::get_default().unwrap();
            icon_theme.append_search_path(&path);
        }
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
            .name("settings").build();
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
        let volume_button = VolumeButtonBuilder::new()
            .name("volume")
            .margin_top(7).margin_bottom(7)
            .value(prefs::get_volume() as f64 / 100.0)
            .build();
        control_box.add(&volume_button);
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
        let playlist_control_box = ButtonBoxBuilder::new()
            .name("meta").layout_style(ButtonBoxStyle::Expand)
            .homogeneous(false)
            .orientation(Orientation::Horizontal).build();
        // make the right edge merge with the window edge :)
        playlist_control_box.pack_end(&BoxBuilder::new().build(), false, false, 0);
        // Button to change shuffle mode:
        let shuffle_button = ToggleButtonBuilder::new()
            .name("shuffle").build();
        playlist_control_box.pack_start(&shuffle_button, false, false, 0);
        // Button to change loop mode:
        let playmode_button = ToggleButtonBuilder::new()
            .name("playmode").build();
        playlist_control_box.pack_start(&playmode_button, false, false, 0);
        // The playlist name:
        let playlist_name = EntryBuilder::new().hexpand(true)
            .build();
        playlist_control_box.pack_start(&playlist_name, true, true, 0);
        // Button to edit playlist settings:
        let playlist_edit_button = ButtonBuilder::new()
            .name("edit_playlist").label("Edit").build();
        playlist_control_box.pack_end(&playlist_edit_button, false, false, 0);
        playlist_itself_box.add(&playlist_control_box);
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
        // done setting up the widgets, time to bind everything to the
        // controller
        let mut scan_thread = ScanThread::new();
        scan_thread.rescan(prefs::get_music_paths())
            .expect("Couldn't start the initial music scan!");
        let playlist_model = None;
        let (playlists_model, _, neu_active_playlist)
            = build_playlists_model(&[]);
        let last_active_playlist = neu_active_playlist;
        let playlist_name_column = TreeViewColumn::new();
        let playlist_name_cell = CellRendererText::new();
        playlist_name_cell.set_property("editable", &true)
            .expect("couldn't make playlist name cell editable");
        playlist_name_column.pack_start(&playlist_name_cell, true);
        playlist_name_column.add_attribute(&playlist_name_cell, "text", 1);
        playlist_name_column.add_attribute(&playlist_name_cell, "weight", 2);
        // I'd love to do this in CSS...
        // (ignore errors because this is not critical to functionality)
        // (fun fact! if this is an f32 instead of an f64, it breaks!)
        let _ = playlist_name_cell.set_property("scale", &0.80);
        let mpris_player = MprisPlayer::new("tsong".to_owned(),
                                            "Tsong".to_owned(),
                                            "tsong".to_owned());
        let nu = Rc::new(RefCell::new(Controller {
            rollup_button, settings_button, prev_button, next_button,
            shuffle_button, playmode_button, play_button, volume_button,
            playlists_view, playlist_view, playlist_name,
            playlists_model, playlist_model, playlist_stats, osd,
            scan_spinner, scan_thread, rollup_grid, control_box,
            new_playlist_button, delete_playlist_button,
            playlist_name_column, playlist_name_cell, window,
            playlist_edit_button,
            mpris_player, mpris_time: 0,
            last_active_playlist, last_active_song: None,
            prev_icon: None, next_icon: None,
            play_icon: None, pause_icon: None,
            rollup_icon: None, rolldown_icon: None, settings_icon: None,
            loop_icon: None, loop_one_icon: None, shuffle_icon: None,
            active_playlist: None, playlist_generation: Default::default(),
            last_built_playlist: None, me: None, settings_controller: None,
            playlist_edit_controller: None, rolled_down_height: 400,
            periodic_timer: None, volume_changed: false,
        }));
        // Throughout this application, we make use of a hack.
        // Each signal that depends on a Controller starts with an attempt to
        // mutably borrow the controller. If said attempt fails, that means
        // that the signal was raised by other code called from within the
        // controller, so we ignore the signal.
        let mut this = nu.borrow_mut();
        this.me = Some(Rc::downgrade(&nu));
        this.settings_controller = Some(settings::Controller::new(Rc::downgrade(&nu)));
        this.playlist_edit_controller = Some(playlist_edit::Controller::new(Rc::downgrade(&nu)));
        this.delete_playlist_button
            .set_sensitive(this.delete_playlist_button_should_be_sensitive());
        this.reload_icons();
        this.playlists_view.append_column(&this.playlist_name_column);
        this.reconnect_playlists_model();
        let controller = nu.clone();
        this.volume_button.connect_value_changed(move |_, value| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.update_volume(value));
        });
        let controller = nu.clone();
        this.playlist_name.connect_property_text_notify(move |_| {
            let _ = controller.try_borrow()
                .map(|x| x.edited_playlist_name_in_entry());
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
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_play());
        });
        this.playlists_view.set_model(Some(&this.playlists_model));
        let controller = nu.clone();
        this.playlist_name_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow()
                .map(|x| x.edited_playlist_name_in_view(wo, nu));
        });
        let controller = nu.clone();
        this.playlists_view.connect_cursor_changed(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.playlists_cursor_changed());
        });
        let controller = nu.clone();
        this.playlist_view.connect_row_activated(move |_, wo, _| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.playlist_row_activated(wo));
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
        let window = this.window.clone();
        this.delete_playlist_button.connect_clicked(move |_| {
            let confirm = MessageDialog::new(Some(&window),
                                             DialogFlags::MODAL,
                                             MessageType::Warning,
                                             ButtonsType::OkCancel,
                                             "Are you sure you want to delete \
                                              the selected playlist?");
            let result = confirm.run();
            confirm.close();
            if result == ResponseType::Cancel { return }
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_playlist());
        });
        let controller = nu.clone();
        this.window.connect_size_allocate(move |_, allocation| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.main_window_resized(allocation));
        });
        let controller = nu.clone();
        this.settings_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_settings());
        });
        let controller = nu.clone();
        this.playlist_edit_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_playlist_edit());
        });
        let controller = nu.clone();
        this.window.connect_key_press_event(move |window, evt| {
            if window.activate_key(evt) { return Inhibit(true) }
            if !window.get_focus().map(|x| x.is::<Entry>()).unwrap_or(false) {
                let keyval = evt.get_keyval();
                use gdk::keys::constants as key;
                match keyval {
                    key::space => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_playpause());
                        return Inhibit(true)
                    },
                    key::Left => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_left());
                        return Inhibit(true)
                    },
                    key::Right => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_right());
                        return Inhibit(true)
                    },
                    // TODO: handle AudioForward and AudioRewind in another way
                    key::AudioCycleTrack | key::AudioForward
                    | key::AudioNext => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_next());
                        return Inhibit(true)
                    },
                    key::AudioRewind | key::AudioPrev => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_prev());
                        return Inhibit(true)
                    },
                    key::AudioLowerVolume => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_quieten());
                        return Inhibit(true)
                    },
                    key::AudioRaiseVolume => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_louden());
                        return Inhibit(true)
                    },
                    key::AudioMute => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_mute());
                        return Inhibit(true)
                    },
                    key::AudioPause => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_pause());
                        return Inhibit(true)
                    },
                    key::AudioPlay => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_play());
                        return Inhibit(true)
                    },
                    key::AudioStop => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_stop());
                        return Inhibit(true)
                    },
                    key::AudioRandomPlay => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_shuffle());
                        return Inhibit(true)
                    },
                    key::AudioRepeat => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.hotkey_playmode());
                        return Inhibit(true)
                    },
                    _ => ()
                }
            }
            return Inhibit(false)
        });
        let controller = nu.clone();
        this.mpris_player.set_can_quit(true);
        this.mpris_player.connect_quit(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_quit());
        });
        let controller = nu.clone();
        this.mpris_player.set_can_raise(true);
        this.mpris_player.connect_raise(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_raise());
        });
        let controller = nu.clone();
        this.mpris_player.set_can_go_next(true);
        this.mpris_player.connect_next(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_next());
        });
        let controller = nu.clone();
        this.mpris_player.set_can_go_previous(true);
        this.mpris_player.connect_previous(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_prev());
        });
        let controller = nu.clone();
        this.mpris_player.set_can_play(true);
        this.mpris_player.connect_play(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_play());
        });
        let controller = nu.clone();
        this.mpris_player.set_can_pause(true);
        this.mpris_player.connect_pause(move || {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_pause());
        });
        // TODO: seek
        //let controller = nu.clone();
        //this.mpris_player.set_can_seek(true);
        let controller = nu.clone();
        this.mpris_player.connect_volume(move |nu| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_set_volume(nu));
        });
        let controller = nu.clone();
        this.mpris_player.connect_shuffle(move |nu| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_set_shuffle(nu));
        });
        let controller = nu.clone();
        this.mpris_player.connect_loop_status(move |nu| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.hotkey_set_playmode(nu.into()));
        });
        this.mpris_player.set_can_control(true);
        this.activate_playlist_by_path(&TreePath::new_first());
        this.force_periodic();
        // okay, show the window and away we go
        this.window.show_all();
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
        let active_song = playback::get_active_song();
        let active_song = active_song.as_ref().map(|x| &x.0);
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
        tvc.add_attribute(&cell, "text", 2);
        tvc.add_attribute(&cell, "weight", 1);
        self.playlist_view.append_column(&tvc);
        let playlist_ref = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => {
                self.playlist_model = None;
                self.playlist_view.set_model::<ListStore>(None);
                self.playlist_generation.destroy();
                self.shuffle_button.set_sensitive(false);
                self.shuffle_button.set_active(false);
                let _ = self.mpris_player.set_shuffle(false);
                self.update_playmode_button();
                return
            },
        };
        let playlist = playlist_ref.maybe_refreshed();
        self.shuffle_button.set_sensitive(true);
        let is_shuffled = playlist.is_shuffled();
        self.shuffle_button.set_active(is_shuffled);
        let _ = self.mpris_player.set_shuffle(is_shuffled);
        self.playlist_generation = playlist.get_playlist_generation();
        let mut types = Vec::with_capacity(playlist.get_columns().len() + 2);
        types.push(SONG_ID_TYPE); // Song ID
        types.push(Type::U32); // Weight of text
        types.push(Type::U32); // Index in playlist
        for _ in playlist.get_columns() {
            types.push(Type::String); // Each metadata column...
        }
        // A bug in GTK+ prevents the built-in sort indicator from being useful
        // so let's just unplug all this code for now.
        /*
        let first_sort_by = playlist.get_sort_order().get(0);
         */
        let playlist_model = ListStore::new(&types[..]);
        let mut column_index: u32 = 3;
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
            /*
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
             */
            tvc.pack_start(&cell, true);
            if column.tag == "duration" || column.tag == "year"
            || column.tag.ends_with("_number") || column.tag.ends_with("#") {
                cell.set_alignment(1.0, 0.5);
                // tvc.set_alignment(1.0);
            }
            tvc.add_attribute(&cell, "text", column_index as i32);
            tvc.add_attribute(&cell, "weight", 1);
            // TODO: i18n this
            column_index += 1;
            self.playlist_view.append_column(&tvc);
        }
        let tvc = TreeViewColumn::new();
        tvc.set_title(""); // blank column to enforce sizes...
        self.playlist_view.append_column(&tvc);
        let mut song_index = 1;
        let mut total_duration = 0u32;
        let mut place_to_put_cursor = None;
        self.last_active_song = active_song.map(|x| {
            (None, x.clone())
        });
        for song_ref in playlist.get_songs() {
            let new_row = playlist_model.append();
            if Some(song_ref) == song_to_select.as_ref() {
                place_to_put_cursor = playlist_model.get_path(&new_row);
            }
            let song = song_ref.read().unwrap();
            playlist_model.set_value(&new_row, 0,
                                     &song_id_to_value(song.get_id()));
            let weight = if Some(song_ref) == active_song {
                // this is a doozy
                match &mut self.last_active_song {
                    Some(x) => x.0 = Some(new_row.clone()),
                    // can't be reached because active_song is non-None, and
                    // therefore last_active_song got set to Some above
                    _ => unreachable!(),
                }
                ACTIVE_WEIGHT
            }
            else { INACTIVE_WEIGHT };
            playlist_model.set_value(&new_row, 1, &weight.to_value());
            playlist_model.set_value(&new_row, 2, &song_index.to_value());
            song_index += 1;
            total_duration = total_duration.saturating_add(song.get_duration());
            let metadata = song.get_metadata();
            let mut column_index: u32 = 3;
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
        let color = self.settings_button.get_style_context()
            .get_color(StateFlags::NORMAL);
        self.prev_icon = get_icon(&color, "tsong-previous");
        self.next_icon = get_icon(&color, "tsong-next");
        self.play_icon = get_icon(&color, "tsong-play");
        self.pause_icon = get_icon(&color, "tsong-pause");
        self.rollup_icon = get_icon(&color, "tsong-rollup");
        self.rolldown_icon = get_icon(&color, "tsong-rolldown");
        self.shuffle_icon = get_icon(&color, "tsong-shuffle");
        self.loop_icon = get_icon(&color, "tsong-loop");
        self.loop_one_icon = get_icon(&color, "tsong-loop-one");
        self.settings_icon = get_icon(&color, "tsong-settings");
        set_image(&self.settings_button, &self.settings_icon,
                  fallback::SETTINGS);
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
        let playlist_ref = match playlist::get_playlist_by_id(id) {
            Some(playlist) => playlist,
            None => {
                eprintln!("Warning: Tried to activate playlist ID {} by path \
                           {}, but it doesn't exist!", id, wo);
                return
            },
        };
        if Some(&playlist_ref) == self.active_playlist.as_ref() {
            return
        }
        self.active_playlist = Some(playlist_ref.clone());
        let _ =
            self.playlist_edit_controller.as_ref().unwrap().try_borrow_mut()
            .map(|mut x| x.activate_playlist(self.active_playlist.as_ref()
                                             .cloned()));
        self.playlist_generation.destroy();
        let playlist = playlist_ref.read().unwrap();
        self.playlist_name.set_text(playlist.get_name());
        drop(playlist);
        self.rebuild_playlist_view();
        let selection = self.playlists_view.get_selection();
        selection.select_path(wo);
    }
    fn periodic(&mut self, forced: bool) {
        self.update_view();
        self.update_scan_status();
        self.maybe_rebuild_playlist();
        if self.volume_changed {
            match prefs::write() {
                Ok(_) => (),
                Err(x) => {
                    eprintln!("Error writing preferences: {:?}", x);
                },
            }
        }
        let timeout_ms =
            if forced || playback::get_playback_status().is_playing() { 100 }
            else { 1000 };
        let controller = match self.me.as_ref().and_then(Weak::upgrade) {
            None => return,
            Some(x) => x,
        };
        self.periodic_timer = Some(timeout_add_local(timeout_ms, move || {
            controller.borrow_mut().periodic(false);
            Continue(false)
        }));
    }
    fn force_periodic(&mut self) {
        match self.periodic_timer.take() {
            Some(x) => source_remove(x),
            None => (),
        }
        self.periodic(true);
    }
    fn change_future_playlist(&mut self, neu: Option<PlaylistRef>) {
        match self.last_active_playlist.as_ref() {
            Some((_, x)) if Some(x) == neu.as_ref() => { return },
            Some((iter, _)) => {
                self.playlists_model.set_value(&iter, 2,
                                               &INACTIVE_WEIGHT.to_value());
            },
            None => (),
        }
        self.last_active_playlist = None;
        match neu.as_ref() {
            Some(neu_ref) => {
                // Do a linear search (ick!) for the correct row to hilight.
                let search_id = neu_ref.read().unwrap().get_id();
                let mut neu_iter = None;
                self.playlists_model.foreach(|model, _, iter| -> bool {
                    let found_id
                        = value_to_playlist_id(model.get_value(&iter, 0));
                    if found_id == Some(search_id) {
                        model.downcast_ref::<TreeStore>().unwrap()
                            .set_value(&iter, 2, &ACTIVE_WEIGHT.to_value());
                        neu_iter = Some(iter.clone());
                        true
                    }
                    else {
                        false
                    }
                });
                if let Some(neu_iter) = neu_iter {
                    self.last_active_playlist
                        = Some((neu_iter, neu_ref.clone()));
                }
            },
            None => (),
        }
        playback::set_future_playlist(neu);
    }
    fn reconnect_playlists_model(&mut self) -> Option<()> {
        let controller = Weak::upgrade(self.me.as_ref().unwrap())?;
        // NOT row-inserted, because that is called before the data is put in
        // so we have no way of knowing which row it was! @_@
        self.playlists_model.connect_row_changed(move |model, path, iter| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.model_playlist_moved(model, path, iter));
        });
        None
    }
    fn model_playlist_moved(&mut self, model: &TreeStore, _path: &TreePath,
                            iter: &TreeIter) -> Option<()> {
        assert_eq!(&self.playlists_model, model);
        let parent_iter = model.iter_parent(iter);
        let sibling_iter = iter.clone();
        let sibling_iter = if model.iter_next(&sibling_iter) {
            Some(sibling_iter)
        } else { None };
        eprintln!("\n\n\nInserted!!!!!!!!!!!!!!!");
        let id = value_to_playlist_id(model.get_value(&iter, 0))?;
        eprintln!("id={:?}", id);
        let fresh = playlist::get_playlist_by_id(id)?;
        eprintln!("fresh={:?}", fresh);
        let parent = parent_iter.and_then(|iter| {
            value_to_playlist_id(model.get_value(&iter, 0))
        }).and_then(playlist::get_playlist_by_id);
        let sibling = sibling_iter.and_then(|iter| {
            value_to_playlist_id(model.get_value(&iter, 0))
        }).and_then(playlist::get_playlist_by_id);
        eprintln!("   parent/sibling: {:?}, {:?}", parent, sibling);
        // make sure all three playlists are unique
        assert_ne!(Some(&fresh), parent.as_ref());
        assert_ne!(Some(&fresh), sibling.as_ref());
        if parent.is_some() && sibling.is_some() {
            assert_ne!(parent, sibling);
        }
        fresh.move_next_to(parent, sibling);
        None
    }
    fn update_view(&mut self) {
        let (status, active_song) = playback::get_status_and_active_song();
        if status.is_playing() {
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
        }
        else {
            set_image(&self.play_button, &self.play_icon, fallback::PLAY);
        }
        let active_song = match active_song {
            None => {
                self.osd.set_label("");
                None
            },
            Some((song_ref, time)) => {
                let song = song_ref.read().unwrap();
                let metadata = song.get_metadata();
                let mpris_time = (time * 1000000.0).floor() as i64;
                if self.mpris_time != mpris_time {
                    self.mpris_time = mpris_time;
                    self.mpris_player.set_position(mpris_time);
                }
                self.osd.set_label
                    (&format!("{} - {}\n{} / {}",
                              metadata.get("title").map(String::as_str)
                              .unwrap_or("Unknown Title"),
                              metadata.get("artist").map(String::as_str)
                              .unwrap_or("Unknown Artist"),
                              pretty_duration(time.floor() as u32),
                              pretty_duration(song.get_duration())));
                drop(song);
                Some(song_ref)
            },
        };
        if self.last_active_song.as_ref().map(|x| &x.1)
        != active_song.as_ref() {
            let playlist_model = self.playlist_model.as_ref().unwrap();
            match self.last_active_song.as_ref() {
                Some((Some(iter), _)) => {
                    playlist_model.set_value(&iter, 1,
                                             &INACTIVE_WEIGHT.to_value());
                },
                _ => (),
            }
            self.last_active_song = active_song.as_ref().map(|x| {
                (None, x.clone())
            });
            match active_song.as_ref() {
                Some(neu_ref) => {
                    // Do a linear search (ick!) for the correct row to
                    // hilight.
                    let search_id = neu_ref.read().unwrap().get_id();
                    let mut neu_iter = None;
                    playlist_model.foreach(|model, _, iter| -> bool {
                        let found_id
                            = value_to_song_id(model.get_value(&iter, 0));
                        if found_id == Some(search_id) {
                            model.downcast_ref::<ListStore>().unwrap()
                                .set_value(&iter, 1,
                                           &ACTIVE_WEIGHT.to_value());
                            neu_iter = Some(iter.clone());
                            true
                        }
                        else {
                            false
                        }
                    });
                    if let Some(neu_iter) = neu_iter {
                        match &mut self.last_active_song {
                            Some(x) => x.0 = Some(neu_iter),
                            _ => (),
                        }
                    }
                },
                None => (),
            }
            let mut mpris_metadata = mpris_player::Metadata {
                length: None,
                art_url: None,
                album: None,
                album_artist: None,
                artist: None,
                composer: None,
                disc_number: None,
                genre: None,
                title: None,
                track_number: None,
                url: None,
            };
            if let Some(song_ref) = active_song.as_ref() {
                let song = song_ref.read().unwrap();
                mpris_metadata.length = Some(song.get_duration() as i64
                                             * 1000000);
                let song_metadata = song.get_metadata();
                mpris_metadata.album = song_metadata.get("album")
                    .map(|x| x.to_owned());
                mpris_metadata.artist = song_metadata.get("artist")
                    .map(|x| vec![x.to_owned()]);
                mpris_metadata.composer = song_metadata.get("composer")
                    .map(|x| vec![x.to_owned()]);
                mpris_metadata.genre = song_metadata.get("genre")
                    .map(|x| vec![x.to_owned()]);
                mpris_metadata.title = song_metadata.get("title")
                    .map(|x| x.to_owned());
                // TODO: parse until first slash, skip spaces
                mpris_metadata.track_number = song_metadata.get("track#")
                    .and_then(|x| x.parse().ok());
                mpris_metadata.disc_number = song_metadata.get("disc#")
                    .and_then(|x| x.parse().ok());
            }
            // TODO: update mpris metadata if we edit the song's metadata while
            // it's playing
            self.mpris_player.set_metadata(mpris_metadata);
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
    fn clicked_play(&mut self) {
        let status = playback::get_playback_status();
        if status.is_playing() {
            playback::send_command(PlaybackCommand::Pause);
            set_image(&self.play_button, &self.play_icon, fallback::PLAY);
        }
        else {
            let song_to_play = if status == PlaybackStatus::Stopped {
                self.change_future_playlist(self.active_playlist.clone());
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x, 0))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            } else { None };
            playback::send_command(PlaybackCommand::Play(song_to_play));
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
            self.force_periodic();
        }
    }
    fn edited_playlist_name_in_view(&self, wo: TreePath,
                                    nu: &str) -> Option<()> {
        let iter = self.playlists_model.get_iter(&wo)?;
        let value = self.playlists_model.get_value(&iter, 0);
        let playlist = value_to_playlist_id(value)
            .and_then(playlist::get_playlist_by_id)?;
        self.playlists_model.set_value(&iter, 1, &Value::from(nu));
        if Some(&playlist) == self.active_playlist.as_ref() {
            self.playlist_name.set_text(&nu);
        }
        playlist.write().unwrap().set_name(nu.to_owned());
        None
    }
    fn edited_playlist_name_in_entry(&self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        let wo = self.playlists_view.get_cursor().0?;
        let iter = self.playlists_model.get_iter(&wo)?;
        // TODO: make sure this is the right playlist!
        let nu = self.playlist_name.get_text().to_string();
        self.playlists_model.set_value(&iter, 1, &nu.to_value());
        playlist.write().unwrap().set_name(nu.to_owned());
        None
    }
    fn playlists_cursor_changed(&mut self) -> Option<()> {
        let wo = self.playlists_view.get_cursor().0?;
        self.activate_playlist_by_path(&wo);
        None
    }
    fn playlist_row_activated(&mut self, wo: &TreePath) -> Option<()> {
        let playlist_model = self.playlist_model.as_ref()?;
        let song = playlist_model.get_iter(wo)
            .map(|x| playlist_model.get_value(&x, 0))
            .and_then(value_to_song_id)
            .and_then(logical::get_song_by_song_id);
        if let Some(song) = song {
            self.change_future_playlist(self.active_playlist.clone());
            playback::send_command(PlaybackCommand::Play(Some(song)));
            self.force_periodic();
        }
        None
    }
    fn clicked_shuffle(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        let now_active = playlist.write().unwrap().toggle_shuffle();
        self.shuffle_button.set_active(now_active);
        let _ = self.mpris_player.set_shuffle(now_active);
        self.rebuild_playlist_view();
        None
    }
    fn clicked_playmode(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        playlist.write().unwrap().bump_playmode();
        self.update_playmode_button();
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
                self.mpris_player.set_loop_status(Playmode::End.into());
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
                self.mpris_player.set_loop_status(playmode.into());
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
            playlists_model.insert_with_values(None, None, &[0, 1, 2],
                                               &[&playlist_id_to_value(id),
                                                 &playlist.get_name(),
                                                 &INACTIVE_WEIGHT]);
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
        let (neu_model, _, neu_active_playlist) = build_playlists_model(&[]);
        self.playlists_model = neu_model;
        self.reconnect_playlists_model();
        self.playlists_view.set_model(Some(&self.playlists_model));
        self.last_active_playlist = neu_active_playlist;
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
    fn clicked_settings(&mut self) -> Option<()> {
        self.settings_controller.as_ref().unwrap().try_borrow_mut().ok()?
            .show();
        None
    }
    fn clicked_playlist_edit(&mut self) -> Option<()> {
        self.playlist_edit_controller.as_ref().unwrap().try_borrow_mut().ok()?
            .show();
        None
    }
    fn rescan(&mut self) {
        match self.scan_thread.rescan(prefs::get_music_paths()) {
            Ok(_) => (),
            Err(x) => eprintln!("Warning: couldn't start music scan! {:?}", x),
        }
        self.force_periodic();
    }
    fn update_volume(&mut self, nu: f64) {
        prefs::set_volume((nu * 100.0).floor() as i32);
        self.volume_changed = true;
    }
    fn edit_playlist(&mut self, neu_code: String,
                     neu_columns: Vec<playlist::Column>) {
        self.active_playlist.as_ref()
            .map(|x| x.write().unwrap()
                 .set_rule_code_and_columns(neu_code, neu_columns));
    }
    fn hotkey_quit(&mut self) {
        self.window.close();
    }
    fn hotkey_raise(&mut self) {
        self.window.present();
    }
    fn hotkey_playpause(&mut self) {
        self.clicked_play()
    }
    fn hotkey_left(&mut self) {
        // TODO: RTL
        self.hotkey_prev()
    }
    fn hotkey_right(&mut self) {
        // TODO: RTL
        self.hotkey_next()
    }
    fn hotkey_prev(&mut self) {
        playback::send_command(PlaybackCommand::Prev)
    }
    fn hotkey_next(&mut self) {
        playback::send_command(PlaybackCommand::Next)
    }
    fn hotkey_quieten(&mut self) {
        let cur_volume = prefs::get_volume();
        let nu_volume = (cur_volume - 5).max(prefs::MIN_VOLUME);
        if cur_volume == nu_volume { return }
        self.volume_button.set_value(nu_volume as f64 / 100.0);
        prefs::set_volume(nu_volume);
        self.volume_changed = true;
    }
    fn hotkey_louden(&mut self) {
        let cur_volume = prefs::get_volume();
        let nu_volume = (cur_volume + 5).max(prefs::MAX_VOLUME);
        if cur_volume == nu_volume { return }
        self.volume_button.set_value(nu_volume as f64 / 100.0);
        prefs::set_volume(nu_volume);
        self.volume_changed = true;
    }
    fn hotkey_mute(&mut self) {
        if playback::toggle_mute() {
            // we are now muted
            self.volume_button.set_value(0.0);
        }
        else {
            // we are no longer muted
            self.volume_button.set_value(prefs::get_volume() as f64 / 100.0);
        }
    }
    fn hotkey_set_volume(&mut self, nu: f64) {
        self.volume_button.set_value(nu);
        let nu = (nu.max(0.0).min(2.0) * 100.0 + 0.5).floor() as i32;
        prefs::set_volume(nu);
        self.volume_changed = true;
    }
    fn hotkey_set_shuffle(&mut self, shuffle: bool) -> Option<()> {
        let playlist_ref = self.active_playlist.as_ref()?;
        let mut playlist = playlist_ref.write().unwrap();
        if playlist.is_shuffled() != shuffle {
            self.shuffle_button.set_active(shuffle);
            playlist.set_shuffle(shuffle);
            drop(playlist);
            // this will result in borrowing the MprisPlayer again. let
            // periodic handle this.
            //self.rebuild_playlist_view();
        }
        None
    }
    fn hotkey_set_playmode(&mut self, nu: Playmode) -> Option<()> {
        let playlist_ref = self.active_playlist.as_ref()?;
        let mut playlist = playlist_ref.write().unwrap();
        if playlist.get_playmode() != nu {
            playlist.set_playmode(nu);
            drop(playlist);
            self.update_playmode_button();
        }
        None
    }
    fn hotkey_pause(&mut self) {
        playback::send_command(PlaybackCommand::Pause)
    }
    fn hotkey_play(&mut self) {
        let status = playback::get_playback_status();
        if status.is_playing() {
            // unlike when PlayPause is clicked, do nothing
        }
        else {
            let song_to_play = if status == PlaybackStatus::Stopped {
                self.change_future_playlist(self.active_playlist.clone());
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x, 0))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            } else { None };
            playback::send_command(PlaybackCommand::Play(song_to_play));
            set_image(&self.play_button, &self.pause_icon, fallback::PAUSE);
            self.force_periodic();
        }
    }
    fn hotkey_stop(&mut self) {
        playback::send_command(PlaybackCommand::Stop)
    }
    fn hotkey_shuffle(&mut self) {
        self.clicked_shuffle();
    }
    fn hotkey_playmode(&mut self) {
        self.clicked_playmode();
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
fn get_icon(color: &RGBA, wat: &str) -> Option<Image> {
    let icon_theme = IconTheme::get_default()?;
    let icon = icon_theme.lookup_icon(wat, 24,
                                      IconLookupFlags::FORCE_SYMBOLIC)?;
    let image = icon.load_symbolic(color, None, None, None).ok()?.0;
    Some(Image::from_pixbuf(Some(&image)))
}

pub fn go() {
    let application = Application::new(
        Some("name.bizna.tsong"),
        Default::default(),
    ).expect("failed to initialize the GTK application (!?!)");
    application.connect_activate(|application| {
        // Controller will hook itself in and keep track of its own lifetime
        let _ = Controller::new(application);
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
                          children: &[PlaylistRef],
                          active_playlist: Option<&PlaylistRef>)
-> Option<(TreeIter,PlaylistRef)> {
    let mut ret = None;
    for playlist_ref in children.iter() {
        let playlist = playlist_ref.read().unwrap();
        let id = playlist.get_id();
        let weight: u32 =
            if Some(playlist_ref) == active_playlist { ACTIVE_WEIGHT }
            else { INACTIVE_WEIGHT };
        let iter
            = playlists_model.insert_with_values(parent_iterator, None,
                                                 &[0, 1, 2],
                                                 &[&playlist_id_to_value(id),
                                                   &playlist.get_name(),
                                                   &weight]);
        if selected_playlists.contains(playlist_ref) {
            match playlists_model.get_path(&iter) {
                Some(x) => selection_paths.push(x),
                None => (),
            }
        }
        if Some(playlist_ref) == active_playlist {
            ret = Some((iter.clone(), playlist_ref.clone()));
        }
        ret = ret.or(add_playlists_to_model(playlists_model,
                                            selected_playlists,
                                            selection_paths,
                                            Some(&iter),
                                            playlist.get_children(),
                                            active_playlist));
    }
    ret
}

/// Returns:
///
/// 1. The new `TreeStore` containing an up to date model of the playlists
/// 2. The new list of paths within the `TreeStore` of selected playlists
///    (excluding any playlists that weren't in the new model)
/// 3. The iterator to the currently active playlist, and a reference to it
fn build_playlists_model(selected_playlists: &[PlaylistRef])
-> (TreeStore, Vec<TreePath>, Option<(TreeIter,PlaylistRef)>) {
    let active_playlist = playback::get_future_playlist();
    let playlists_model = TreeStore::new(&[PLAYLIST_ID_TYPE,Type::String,
                                           Type::U32]);
    assert!(playlists_model.get_flags()
            .contains(TreeModelFlags::ITERS_PERSIST));
    let mut selection_paths = Vec::with_capacity(selected_playlists.len());
    let neu_active_playlist =
        add_playlists_to_model(&playlists_model, selected_playlists,
                               &mut selection_paths, None,
                               &playlist::get_top_level_playlists()[..],
                               active_playlist.as_ref());
    (playlists_model, selection_paths, neu_active_playlist)
}

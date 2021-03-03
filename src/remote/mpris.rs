use crate::*;

use mpris_player::{
    MprisPlayer,
    OrgMprisMediaPlayer2Player,
};

use std::{
    cell::RefCell,
    sync::Arc,
    rc::{Weak},
};

pub struct MprisRemote {
    mpris_player: Arc<MprisPlayer>
}

impl MprisRemote {
    pub fn new<T: 'static + RemoteTarget>(remote: Weak<RefCell<T>>) -> MprisRemote {
        let mpris_player = MprisPlayer::new("tsong".to_owned(),
                                            "Tsong".to_owned(),
                                            "tsong".to_owned());
        let weak = remote.clone();
        mpris_player.set_can_quit(true);
        mpris_player.connect_quit(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_quit()));
        });
        let weak = remote.clone();
        mpris_player.set_can_raise(true);
        mpris_player.connect_raise(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_raise()));
        });
        let weak = remote.clone();
        mpris_player.set_can_go_next(true);
        mpris_player.connect_next(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_next()));
        });
        let weak = remote.clone();
        mpris_player.set_can_go_previous(true);
        mpris_player.connect_previous(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_prev()));
        });
        let weak = remote.clone();
        mpris_player.set_can_play(true);
        mpris_player.connect_play(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_play()));
        });
        let weak = remote.clone();
        mpris_player.set_can_pause(true);
        mpris_player.connect_pause(move || {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_pause()));
        });
        // TODO: seek
        //let weak = remote.clone();
        //mpris_player.set_can_seek(true);
        let weak = remote.clone();
        mpris_player.connect_volume(move |nu| {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_set_volume(nu)));
        });
        let weak = remote.clone();
        mpris_player.connect_shuffle(move |nu| {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_set_shuffle(nu)));
        });
        let weak = remote.clone();
        mpris_player.connect_loop_status(move |nu| {
            let _ = weak.upgrade().and_then(|x| x.try_borrow_mut().ok()
                .map(|mut x| x.remote_set_playmode(nu.into())));
        });
        mpris_player.set_can_control(true);
        MprisRemote {
            mpris_player
        }
    }
}

impl super::RemoteSource for MprisRemote {
    fn set_play_pos(&self, pos: f64) {
        self.mpris_player.set_position((pos * 1000000.0).floor() as i64);
    }
    fn set_is_shuffled(&self, is_shuffled: bool) {
        let _ = self.mpris_player.set_shuffle(is_shuffled);
    }
    fn set_cur_playmode(&self, playmode: Playmode) {
        self.mpris_player.set_loop_status(playmode.into());
    }
    fn set_now_playing(&self, song_ref: Option<&LogicalSongRef>) {
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
        if let Some(song_ref) = song_ref {
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
        self.mpris_player.set_metadata(mpris_metadata);
    }
}

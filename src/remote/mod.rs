use crate::*;

#[cfg(feature="mpris")]
mod mpris;

use std::{
    cell::RefCell,
    rc::Weak,
};

pub struct Remote {
    #[cfg(feature="mpris")]
    mpris: mpris::MprisRemote,
}

// TODO: currently this interface assumes only one kind of remote is active at
// once...
impl Remote {
    pub fn new<T: 'static + RemoteTarget>(target: Weak<RefCell<T>>) -> Remote {
        #[cfg(not(feature="mpris"))]
        let _ = target;
        Remote {
            #[cfg(feature="mpris")]
            mpris: mpris::MprisRemote::new(target)
        }
    }
    pub fn set_now_playing(&self, song: Option<&LogicalSongRef>) {
        #[cfg(not(feature="mpris"))]
        let _ = song;
        #[cfg(feature="mpris")]
        self.mpris.set_now_playing(song);
    }
    pub fn set_play_pos(&self, pos: f64) {
        #[cfg(not(feature="mpris"))]
        let _ = pos;
        #[cfg(feature="mpris")]
        self.mpris.set_play_pos(pos);
    }
    pub fn set_is_shuffled(&self, is_shuffled: bool) {
        #[cfg(not(feature="mpris"))]
        let _ = is_shuffled;
        #[cfg(feature="mpris")]
        self.mpris.set_is_shuffled(is_shuffled);
    }
    pub fn set_cur_playmode(&self, playmode: Playmode) {
        #[cfg(not(feature="mpris"))]
        let _ = playmode;
        #[cfg(feature="mpris")]
        self.mpris.set_cur_playmode(playmode);
    }
}

pub trait RemoteTarget {
    fn remote_quit(&mut self) -> Option<()>;
    fn remote_raise(&mut self) -> Option<()>;
    fn remote_playpause(&mut self) -> Option<()>;
    fn remote_left(&mut self) -> Option<()>;
    fn remote_right(&mut self) -> Option<()>;
    fn remote_prev(&mut self) -> Option<()>;
    fn remote_next(&mut self) -> Option<()>;
    fn remote_quieten(&mut self) -> Option<()>;
    fn remote_louden(&mut self) -> Option<()>;
    fn remote_mute(&mut self) -> Option<()>;
    fn remote_set_volume(&mut self, nu: f64) -> Option<()>;
    fn remote_set_shuffle(&mut self, shuffle: bool) -> Option<()>;
    fn remote_set_playmode(&mut self, nu: Playmode) -> Option<()>;
    fn remote_pause(&mut self) -> Option<()>;
    fn remote_play(&mut self) -> Option<()>;
    fn remote_stop(&mut self) -> Option<()>;
    fn remote_shuffle(&mut self) -> Option<()>;
    fn remote_playmode(&mut self) -> Option<()>;
}

trait RemoteSource {
    fn set_now_playing(&self, _song: Option<&LogicalSongRef>);
    fn set_play_pos(&self, _pos: f64);
    fn set_is_shuffled(&self, _is_shuffled: bool);
    fn set_cur_playmode(&self, _playmode: Playmode);
}

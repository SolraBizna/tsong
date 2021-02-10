use crate::*;

pub fn load_database() -> anyhow::Result<()> {
    let _ = playlist::add_playlist_from_db(PlaylistID::from_inner(3), None, 3,
                                           "One Song".to_owned(),
                                           "".to_owned(),
                                           false,
                                           vec![SongID::from_inner(14)
                                           ],
                                           playlist::DEFAULT_COLUMNS.clone(),
                                           vec![]);
    let any = playlist::add_playlist_from_db(PlaylistID::from_inner(1), None,0,
                                             "All Songs".to_owned(),
                                             "any".to_owned(),
                                             false,
                                             vec![],
                                             playlist::DEFAULT_COLUMNS.clone(),
                                             vec![]);
    playback::set_future_playlist(Some(any));
    let _ = playlist::add_playlist_from_db(PlaylistID::from_inner(2), None, 1,
                                           "Unchecked Songs".to_owned(),
                                           "unchecked:set()".to_owned(),
                                           false,
                                           vec![],
                                           playlist::DEFAULT_COLUMNS.clone(),
                                           vec![]);
    let mut mck_columns = playlist::DEFAULT_COLUMNS.clone();
    mck_columns.retain(|x| x.tag != "artist");
    let _ = playlist::add_playlist_from_db(PlaylistID::from_inner(4), None, 9,
                                           "McKennitt".to_owned(),
                                           "artist:contains \"McKennitt\""
                                           .to_owned(),
                                           false,
                                           vec![],
                                           mck_columns.clone(),
                                           vec![("title".to_owned(),false)]);
    let _ = playlist::add_playlist_from_db(PlaylistID::from_inner(5),
                                           Some(PlaylistID::from_inner(4)),
                                           2,
                                           "The \"The\" Songs".to_owned(),
                                           "artist:contains \"McKennitt\" \
                                            and title:starts_with \"The\""
                                           .to_owned(),
                                           false,
                                           vec![],
                                           mck_columns.clone(),
                                           vec![("title".to_owned(),false)]);
    let _ = playlist::add_playlist_from_db(PlaylistID::from_inner(6),
                                           Some(PlaylistID::from_inner(4)),
                                           1,
                                           "The Santiagos".to_owned(),
                                           "artist:contains \"McKennitt\" \
                                            and title:contains \"Santiago\""
                                           .to_owned(),
                                           false,
                                           vec![],
                                           mck_columns,
                                           vec![("title".to_owned(),false)]);
    playlist::rebuild_children();
    Ok(())
}

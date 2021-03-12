use gtk::{
    prelude::*,
    Button,
    //IconTheme,
    //IconLookupFlags,
    IconSize,
    Image,
};
use gdk::RGBA;

use std::{
    collections::HashMap,
};

enum IconOrFallback {
    Icon(Image),
    Fallback(&'static str),
}

/// `(icon name, fallback text)`
const ICONS: &[(&str, &str)] = &[
    ("tsong-rollup", "\u{1F783}\u{FE0E}"),
    ("tsong-rolldown", "\u{1F781}\u{FE0E}"),
    ("tsong-settings", "\u{2699}\u{FE0E}"),
    ("tsong-shuffle", "\u{1F500}\u{FE0E}"),
    ("tsong-loop", "\u{1F501}\u{FE0E}"),
    ("tsong-loop-one", "\u{1F502}\u{FE0E}"),
    ("tsong-prev", "\u{1F844}\u{FE0E}"),
    ("tsong-next", "\u{1F846}\u{FE0E}"),
    ("tsong-add", "\u{FF0B}\u{FE0E}"),
    ("tsong-remove", "\u{FF0D}\u{FE0E}"),
    ("tsong-reset", "\u{21BB}\u{FE0E}"),
    ("tsong-play", "\u{23F5}\u{FE0E}"),
    ("tsong-pause", "\u{23F8}\u{FE0E}"),
];

#[derive(Default)]
pub struct Icons {
    icons: HashMap<String, IconOrFallback>,
    buttons: Vec<(Button, &'static str)>,
}

fn get_icon_image(scale_factor: i32, color: &RGBA, name: &str)
-> Option<Image> {
    /*
    let icon_theme = IconTheme::get_default()?;
    let icon = icon_theme.lookup_icon_for_scale(name, 24, scale_factor,
                                                IconLookupFlags
                                                ::FORCE_SYMBOLIC)?;
    let image = icon.load_symbolic(color, None, None, None).ok()?.0;
    Some(Image::from_pixbuf(Some(&image)))
     */
    Some(Image::from_icon_name(Some(name), IconSize::LargeToolbar))
}

fn get_icon(scale_factor: i32, color: &RGBA, name: &str,
            fallback: &'static str) -> IconOrFallback {
    match get_icon_image(scale_factor, color, name) {
        Some(x) => IconOrFallback::Icon(x),
        None => IconOrFallback::Fallback(fallback),
    }
}

fn set_image(button: &Button, icon: &IconOrFallback) {
    match icon {
        IconOrFallback::Icon(icon) => {
            button.set_image(Some(icon));
            button.set_label("");
            let _ = button.set_property("always-show-image", &true);
        },
        IconOrFallback::Fallback(fallback) => {
            button.set_image::<Image>(None);
            button.set_label(fallback);
            let _ = button.set_property("always-show-image", &false);
        },
    }
}

impl Icons {
    pub fn reload_icons(&mut self, scale_factor: i32, color: &RGBA) {
        // TODO: reload icons when theme is changed
        self.icons.clear();
        for (name, fallback) in ICONS {
            self.icons.insert(name.to_string(),
                              get_icon(scale_factor, color, name, fallback));
        }
        for (button, icon) in self.buttons.iter() {
            let got = self.icons.get(*icon)
                .unwrap_or(&IconOrFallback::Fallback(""));
            set_image(&button, got);
        }
    }
    pub fn set_icon<B: IsA<Button>>(&mut self, button: &B, icon: &'static str){
        let button = button.upcast_ref();
        let got = self.icons.get(icon)
            .unwrap_or(&IconOrFallback::Fallback(""));
        for it in self.buttons.iter_mut() {
            if &it.0 == button {
                if it.1 != icon {
                    it.1 = icon;
                    set_image(button, got);
                    return
                }
            }
        }
        // adding a new one
        self.buttons.push((button.clone(), icon));
        set_image(button, got);
    }
}

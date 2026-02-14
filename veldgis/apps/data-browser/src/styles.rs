use veld_ui::{Style, ButtonStyle, Appearance, Color, Border};

pub fn file_button() -> Style {
    let base = Appearance {
        background: Some(Color::from_rgb(0.2, 0.2, 0.25)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    
    let hovered = Appearance {
        background: Some(Color::from_rgb(0.25, 0.25, 0.35)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: Appearance::default(),
    }.into()
}

pub fn sync_button() -> Style {
    let base = Appearance {
        background: Some(Color::from_rgb(0.15, 0.15, 0.2)),
        text_color: Some(Color::from_rgb(0.8, 0.8, 0.8)),
        border: Border {
            color: Color::from_rgb(0.3, 0.3, 0.4),
            width: 1.0,
            radius: 4.0,
        },
        ..Default::default()
    };
    
    let hovered = Appearance {
        background: Some(Color::from_rgb(0.2, 0.2, 0.3)),
        text_color: Some(Color::WHITE),
        border: Border {
            color: Color::from_rgb(0.5, 0.5, 0.7),
            width: 1.0,
            radius: 4.0,
        },
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: Appearance::default(),
    }.into()
}

pub fn download_button() -> Style {
    let base = Appearance {
        background: Some(Color::from_rgb(0.1, 0.3, 0.1)),
        text_color: Some(Color::WHITE),
        border: Border {
            color: Color::from_rgb(0.2, 0.5, 0.2),
            width: 1.0,
            radius: 4.0,
        },
        ..Default::default()
    };
    
    let hovered = Appearance {
        background: Some(Color::from_rgb(0.15, 0.4, 0.15)),
        text_color: Some(Color::WHITE),
        border: Border {
            color: Color::from_rgb(0.3, 0.7, 0.3),
            width: 1.0,
            radius: 4.0,
        },
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: Appearance::default(),
    }.into()
}

use veld_ui::{Style, ButtonStyle, Appearance, Color, Border};

pub fn file_button() -> Style {
    let base = Appearance {
        background: Some(Color::from_rgb(0.2, 0.2, 0.25)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    
    let hovered = Appearance {
        background: Some(Color::from_rgb(0.3, 0.3, 0.35)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base, // Simplified
        disabled: Appearance {
             text_color: Some(Color::from_rgb(0.5, 0.5, 0.5)),
             ..Default::default()
        },
    }.into()
}

pub fn sync_button() -> Style {
    "text".into() // Fallback to string class for simple text buttons
}

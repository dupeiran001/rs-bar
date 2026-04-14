use super::Theme;

pub const NORD: Theme = Theme {
    bg:               0x2E3440, // polar night 1
    bg_dark:          0x272C36, // darker shade
    bg_dark1:         0x222730, // darkest shade
    fg:               0xECEFF4, // snow storm 3
    fg_dark:          0xD8DEE9, // snow storm 1
    fg_gutter:        0x4C566A, // polar night 4
    surface:          0x3B4252, // polar night 2
    text_dim:         0xD8DEE9, // snow storm 1
    accent:           0x88C0D0, // frost 2
    accent_dim:       0x81A1C1, // frost 3
    green:            0xA3BE8C, // aurora green
    yellow:           0xEBCB8B, // aurora yellow
    orange:           0xD08770, // aurora orange
    red:              0xBF616A, // aurora red
    blue:             0x5E81AC, // frost 4
    teal:             0x8FBCBB, // frost 1
    purple:           0xB48EAD, // aurora purple
    border:           0x3B4252, // same as surface — capsules render borderless
    border_highlight: 0x434C5E, // polar night 3
    error:            0xBF616A, // aurora red
    warn:             0xEBCB8B, // aurora yellow
    info:             0x88C0D0, // frost 2
};

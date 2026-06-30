//! Russ is modal, and these are the modes it can be in.

/// what type of object is currently selected
#[derive(Clone, Debug)]
pub enum Selected {
    Feeds,
    Entries,
    Entry(crate::rss::EntryMetadata),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationAction {
    ClearCache,
    ClearChat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Editing,
    Normal,
    Command,
    Settings,
    SettingsEditing(usize),
    ViewLlmLog,
    Confirmation(ConfirmationAction),
    /// Full-screen chat about the article with the given entry id.
    Chat(crate::rss::EntryId),
}

#[derive(Clone, Debug)]
pub enum ReadMode {
    ShowRead,
    ShowUnread,
    All,
}

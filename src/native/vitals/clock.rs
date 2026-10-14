//! Local clock, adapted from Vitals under its MIT license.
use std::{
    mem::MaybeUninit,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}
impl LocalTime {
    pub fn date(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
    pub fn time(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}
pub fn at(seconds: libc::time_t) -> Option<LocalTime> {
    let mut tm = MaybeUninit::<libc::tm>::uninit();
    // SAFETY: valid time_t pointer and writable libc-defined tm buffer.
    // localtime_r uses the server's inherited TZ, or /etc/localtime.
    if unsafe { libc::localtime_r(&seconds, tm.as_mut_ptr()) }.is_null() {
        return None;
    }
    // SAFETY: localtime_r succeeded and initialized the entire structure.
    let tm = unsafe { tm.assume_init() };
    Some(LocalTime {
        year: tm.tm_year.checked_add(1900)?,
        month: u8::try_from(tm.tm_mon + 1).ok()?,
        day: u8::try_from(tm.tm_mday).ok()?,
        hour: u8::try_from(tm.tm_hour).ok()?,
        minute: u8::try_from(tm.tm_min).ok()?,
    })
}
pub fn now() -> Option<LocalTime> {
    at(
        libc::time_t::try_from(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs())
            .ok()?,
    )
}

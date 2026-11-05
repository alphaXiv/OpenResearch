//! §7 `notify` notices: new bytes of a run's `$ORX_NOTIFY_FILE`, at most one per run per
//! 10 minutes. Bytes coalesce inside the window; final collection bypasses it.

use std::time::Duration;

use super::bus::RunEvent;
use super::{handle_of, phase_of, Backend, ByteRange, Engine, LogFile, LogPage, WatchedRun};
use crate::error::{anyhow, Result};
use crate::store::{NotifyCursor, NOTIFY};

const NOTIFY_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// A notice carries at most this tail of the new bytes; the file itself keeps the rest.
const NOTIFY_BYTES: u64 = 4 * 1024;

impl Engine {
    fn notify_window_open(&self, run_id: &str) -> Result<bool> {
        let interval = i64::try_from(NOTIFY_INTERVAL.as_millis())?;
        let last = self.store().last_notice_at(run_id, NOTIFY)?;
        Ok(last.is_none_or(|last| self.clock.wall_ms() - last >= interval))
    }

    /// Runs with reported notify bytes whose rate window is open.
    pub fn notify_due(&self) -> Result<Vec<String>> {
        let pending: Vec<String> = self.memory().notify.keys().cloned().collect();
        let mut due = Vec::new();
        for run_id in pending {
            if self.notify_window_open(&run_id)? {
                due.push(run_id);
            }
        }
        Ok(due)
    }

    /// Turns the bytes past the run's notify cursor into one notice per subscribed chat, then
    /// advances the cursor in the same transaction.
    pub async fn deliver_notify(&self, backend: &dyn Backend, run_id: &str) -> Result<bool> {
        let Some(_gate) = self.try_gate(run_id) else {
            return Ok(false);
        };
        let (run, state) = self.load(run_id)?;
        let terminal = phase_of(&run)?.is_terminal();
        if !terminal && (!self.notify_window_open(run_id)? || !self.own(run_id)?) {
            return Ok(false);
        }
        let Some(handle) = handle_of(&run) else {
            return Ok(true);
        };
        let reported = self.memory().notify.get(run_id).copied();
        if !terminal && reported.is_none() {
            return Ok(false);
        }
        let watched = WatchedRun {
            id: run.id.clone(),
            handle,
        };
        let mut len = if terminal {
            match backend
                .read_log(
                    &watched,
                    LogFile::Notify,
                    ByteRange {
                        start: u64::MAX - 1,
                        end: u64::MAX,
                    },
                    None,
                )
                .await
            {
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::Unsupported) =>
                {
                    return Ok(true)
                }
                Err(error) => return Err(error),
                Ok(LogPage::Reset { info: Some(info) }) => info.len,
                Ok(LogPage::Reset { info: None }) => {
                    self.memory().notify.remove(run_id);
                    return Ok(true);
                }
                Ok(LogPage::Data { .. }) => {
                    return Err(anyhow!("The host answered a notify length probe with data"))
                }
            }
        } else {
            reported.unwrap()
        };
        let tail = |from: u64, end: u64| ByteRange {
            start: from.max(end.saturating_sub(NOTIFY_BYTES)),
            end,
        };

        // A file now shorter than the cursor was truncated or replaced, whatever its identity.
        let old = state.notify_cursor;
        let continued = old.as_ref().filter(|cursor| cursor.offset <= len);
        let mut offset = continued.map_or(0, |cursor| cursor.offset);
        let mut same_file = continued.is_some();
        let ident = continued.map(|cursor| &cursor.ident);
        let mut page = backend
            .read_log(&watched, LogFile::Notify, tail(offset, len), ident)
            .await?;
        if let LogPage::Reset { info } = page {
            let Some(info) = info else {
                self.memory().notify.remove(run_id);
                return Ok(true);
            };
            (offset, same_file) = (0, false);
            len = info.len;
            page = backend
                .read_log(
                    &watched,
                    LogFile::Notify,
                    tail(0, info.len),
                    Some(&info.ident),
                )
                .await?;
        }
        let LogPage::Data {
            ident,
            start,
            bytes,
            eof,
            ..
        } = page
        else {
            return Err(anyhow!("The notify file changed during collection"));
        };
        let end = start + bytes.len() as u64;
        let generation = match &old {
            Some(old) if same_file => old.generation,
            Some(old) => old.generation + 1,
            None => 0,
        };
        let cursor = NotifyCursor {
            ident,
            offset: end,
            generation,
        };

        let mut chats = Vec::new();
        if old.as_ref() != Some(&cursor) {
            let omitted = start.saturating_sub(offset);
            let text = String::from_utf8_lossy(&bytes);
            let mut body = format!("[orx] Run `{run_id}` wrote to its notify file:\n{text}");
            if omitted > 0 {
                body.push_str(&format!(
                    "\n({omitted} earlier bytes omitted; see the file.)"
                ));
            }
            let now = self.clock.wall_ms();
            let dedupe = format!("{NOTIFY}:{generation}:{end}");
            let store = self.store();
            let tx = store.begin_immediate()?;
            store.set_notify_cursor(run_id, &cursor)?;
            for chat in store.run_subscribers(run_id)? {
                if !bytes.is_empty()
                    && store.insert_run_notice(run_id, &chat, NOTIFY, &dedupe, &body, now)?
                {
                    chats.push(chat);
                }
            }
            tx.commit()?;
        }
        {
            let mut memory = self.memory();
            // The length this read started from is consumed even if the file has since shrunk;
            // a newer report stays pending.
            if memory
                .notify
                .get(run_id)
                .is_some_and(|pending| *pending == reported.unwrap_or(len) || *pending <= end)
            {
                memory.notify.remove(run_id);
            }
        }
        if !chats.is_empty() {
            self.bus.publish(RunEvent::Notices { chats });
        }
        Ok(end >= len && (!terminal || eof))
    }
}

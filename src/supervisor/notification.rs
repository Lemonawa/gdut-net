//! Supervisor 内部通知账本：在飞去重与成功投递后的节流窗口。

use crate::supervisor::{Effect, ToastKey};

const NOTIFY_THROTTLE_MS: u64 = 30 * 60 * 1000;

#[derive(Default)]
pub(crate) struct NotificationLedger {
    in_flight: Vec<ToastKey>,
    delivered_at: Vec<(ToastKey, u64)>,
}

impl NotificationLedger {
    pub(crate) fn on_result(&mut self, key: ToastKey, delivered: bool, now: u64) {
        let Some(pos) = self.in_flight.iter().position(|k| *k == key) else {
            return; // 迟到的投递结果：no-op
        };
        self.in_flight.remove(pos);
        if delivered {
            // 窗口只在成功投递后开启（失败不占用节流窗口）。
            self.delivered_at.retain(|(k, _)| *k != key);
            self.delivered_at.push((key, now));
        }
    }

    pub(crate) fn maybe_notify(
        &mut self,
        key: ToastKey,
        title: &str,
        body: String,
        now: u64,
        effects: &mut Vec<Effect>,
    ) {
        if self.in_flight.contains(&key) {
            return;
        }
        if let Some(&(_, at)) = self.delivered_at.iter().find(|(k, _)| *k == key) {
            if now.saturating_sub(at) < NOTIFY_THROTTLE_MS {
                return;
            }
        }
        log::info!("Toast [{}]: {title} — {body}", key.as_str());
        self.in_flight.push(key);
        effects.push(Effect::Notify {
            key,
            title: title.to_string(),
            body,
        });
    }
}

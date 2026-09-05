-- Heartbeats use the existing notification outbox; no separate scheduler state.
ALTER TABLE brunn.notifications
  DROP CONSTRAINT notifications_kind_check,
  ADD CONSTRAINT notifications_kind_check CHECK (
    kind IN (
      'briefing_ready', 'news_alert', 'correction', 'operational', 'task_guard',
      'location_heartbeat'
    )
  );

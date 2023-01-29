# pa_queue_and_chan
Some different type of queues and chans.
## NOTICE

Async logic maybe support tokio, tokio waker is transplanted in and used.
## priority async chan

Elements in the chan are sorted by priority, the lower the priority, the higher the priority.
## kv_mpsc & kv_mpsc_multikey

They are sync mpsc, and kv for msg conflict, newer key msg can be took out only when there's no same key holding by msg.
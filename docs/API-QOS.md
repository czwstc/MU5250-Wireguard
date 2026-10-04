# GET /api/modem/qos

Authenticated read-only Agent endpoint, standard `{ "ok": true, "data": ... }` envelope.
Bearer credentials use the same authentication as `/api/dashboard`. No modem commands or configuration writes.

`data.status`: `available`, `no_data`, `unsupported`, or `error` (no successful sample yet).
`sampled_at`: Unix seconds of the last successful sample, nullable.
`source`: source identifier; B31 uses `uci:zwrt_data_tmp.wwaniface*.qci/5qi`.
`stale`: true after a failed refresh; the last successful sample is retained.
`error`: nullable diagnostic string. Cache TTL is three seconds; subprocesses use the Agent's bounded runner, and network technology shares its radio cache.

`bearers` contains connected contexts, each with `id`, numeric `cid`, nullable `apn`, nullable `technology`, nullable `interface`, `connected`, nullable `default_data`, nullable integer `qci` and `five_qi`. Default data is established by the IPv4 default route interface. Unknown values remain null. Disconnected contexts are omitted, even when firmware retains an old QoS field. Values must be integers 1–255.

B31 evidence on 2026-10-04: `zwrt_data_tmp.wwaniface1.connect_status=ipv4_ipv6_connected`, `qci=6`, `ipv4_dev_name=rmnet_data0`, matching the default route. Radio reported SA. The endpoint returns QCI 6, never infers 5QI from SA; `five_qi` and the unavailable per-context APN are null. No IMEI, IMSI, ICCID, passwords or session tokens are included in this record.

Older Agents return 404; the native client asks for an upgrade. Unsupported, no data, errors and stale samples have separate native states. The dashboard API contract and mock include this endpoint; mock QoS is explicitly synthetic.

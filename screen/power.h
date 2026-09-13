/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Event timestamps, not poll time: queued long presses must remain long. */
struct power_button { long long pressed, released; int down, dropped; };
static int power_event(struct power_button *k, int type, int code, int value,
                       long long stamp) {
  if (type == 0 && code == 3) { /* SYN_DROPPED */
    k->down = 0; k->dropped = 1; return 0;
  }
  if (k->dropped) {
    if (type == 0 && code == 0) k->dropped = 0;
    return 0;
  }
  if (type != 1 || code != 116) return 0; /* EV_KEY, KEY_POWER */
  if (value == 1 && !k->down) { k->down = 1; k->pressed = stamp; }
  if (value != 0 || !k->down) return 0;
  k->down = 0;
  long long duration = stamp - k->pressed;
  int short_press = duration >= 30 && duration < 800 &&
                    (!k->released || k->pressed - k->released >= 80);
  k->released = stamp;
  return short_press;
}
static void power_self_test(void) {
  struct power_button k = {0};
  assert(!power_event(&k, 1, 116, 0, 100)); /* Opened while held */
  assert(!power_event(&k, 1, 116, 1, 1000));
  assert(!power_event(&k, 1, 116, 2, 1100)); /* Repeat */
  assert(power_event(&k, 1, 116, 0, 1200));
  assert(!power_event(&k, 1, 116, 0, 1201));
  assert(!power_event(&k, 1, 116, 1, 1220));
  assert(!power_event(&k, 1, 116, 0, 1260)); /* Bounce */
  assert(!power_event(&k, 1, 116, 1, 2000));
  assert(!power_event(&k, 1, 116, 0, 2800)); /* Long press */
  assert(!power_event(&k, 1, 116, 1, 3000));
  assert(!power_event(&k, 0, 3, 0, 3050));
  assert(!power_event(&k, 1, 116, 0, 3100));
  assert(!power_event(&k, 0, 0, 0, 3200));
  assert(!power_event(&k, 1, 116, 0, 3300));
  assert(!power_event(&k, 1, 116, 1, 4000));
  assert(power_event(&k, 1, 116, 0, 4100));
}

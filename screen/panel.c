/* SPDX-License-Identifier: GPL-3.0-or-later
 * WireGuard touch UI; native display and root IPC only. See LICENSE and NOTICE.
 */
#define _GNU_SOURCE
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifndef SCREEN_PREVIEW
#include "qpic.h"
#include <dirent.h>
#include <sys/file.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#else
#define W 320
#define H 480
struct drm_buf {
  uint16_t *map;
  uint32_t pitch;
};
static void fill(struct drm_buf *b, uint16_t c) {
  for (int i = 0; i < W * H; i++)
    b->map[i] = c;
}
static void fill_rect(struct drm_buf *b, int x, int y, int x1, int y1,
                      uint16_t c) {
  for (int j = y; j < y1; j++)
    for (int i = x; i < x1; i++)
      if (i >= 0 && i < W && j >= 0 && j < H)
        b->map[j * W + i] = c;
}
static void draw_pixel(struct drm_buf *b, int x, int y, uint16_t c) {
  if (x >= 0 && x < W && y >= 0 && y < H)
    b->map[y * W + x] = c;
}
#endif
#include "power.h"
#include "devui/html.h"
#include "devui_assets.h"
#define MAX_ROWS 128
#define BG 0x0863
#define CARD 0x1926
#define FG 0xef9e
#define MUTED 0x9d55
#define GREEN 0x46f3
#define BLUE 0x253b
#define RED 0xfa8a
#define RUNDIR "/tmp/openui-screen"
#define BL "/sys/class/leds/led:lcd/brightness"
struct row {
  char mac[18], ip[48], name[193];
  int selected, online;
};
struct profile { unsigned long long id; int active; char name[193]; };
struct snapshot {
  unsigned long long rev, handshake, rx, tx, stamp, job, done;
  int has, enabled, connected, error, all, busy, result, n;
  int np, battery, temperature;
  unsigned long long uptime;
  double memory, cpu;
  struct profile profiles[5];
  struct row rows[MAX_ROWS];
};
static struct snapshot data, draft;
static int original_selection[MAX_ROWS];
#ifndef SCREEN_PREVIEW
static int screen_awake = 1;
#endif
enum { WG, DEVICES, MENU, PROFILES, OVERVIEW };
static int page = MENU, offset, dialog, dirty, quitting, need_draw = 1,
                                                  return_after_save;
static unsigned long long pending, pending_revision, chosen_profile, chosen_revision;
static char chosen_name[193];
static char notice[128];
static long long monotonic_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (long long)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}
static void amount(char *out, size_t n, unsigned long long v) {
  if (v >= 1073741824)
    snprintf(out, n, "%.1f GB", (double)v / 1073741824);
  else if (v >= 1048576)
    snprintf(out, n, "%.1f MB", (double)v / 1048576);
  else
    snprintf(out, n, "%.1f KB", (double)v / 1024);
}
static int stale(void) {
  return !data.stamp || (unsigned long long)time(NULL) > data.stamp + 10;
}
static int blocked(void) { return pending || data.busy || stale(); }
#include "devui/render.h"
// The LCD and raw touch axes are mounted upside down relative to the case.
// Keep layout coordinates upright; transform only at the hardware boundary.
static void rotate_point(int *x, int *y) {
  *x = W - 1 - *x;
  *y = H - 1 - *y;
}
static void rotate_frame(struct drm_buf *b) {
  for (int y = 0; y < H / 2; y++) {
    uint16_t *top = (uint16_t *)((uint8_t *)b->map + y * b->pitch);
    uint16_t *bottom = (uint16_t *)((uint8_t *)b->map + (H - 1 - y) * b->pitch);
    for (int x = 0; x < W; x++) {
      uint16_t value = top[x];
      top[x] = bottom[W - 1 - x];
      bottom[W - 1 - x] = value;
    }
  }
}
static int parse_snapshot(char *buf, struct snapshot *out) {
  struct snapshot s = {.battery = -1, .temperature = -999, .memory = -1};
  char *save, *line = strtok_r(buf, "\n", &save);
  int valid = 0;
  while (line) {
    if (line[0] == 'S' &&
        sscanf(line, "S %llu %d %d %d %d %llu %llu %llu %d %llu", &s.rev,
               &s.has, &s.enabled, &s.connected, &s.error, &s.handshake, &s.rx,
               &s.tx, &s.all, &s.stamp) == 10)
      valid = 1;
    else if (line[0] == 'J')
      sscanf(line, "J %d %llu %llu %d", &s.busy, &s.job, &s.done, &s.result);
    else if (line[0] == 'O')
      sscanf(line, "O %d %d %llu %lf %lf", &s.battery, &s.temperature, &s.uptime, &s.memory, &s.cpu);
    else if (line[0] == 'P' && s.np < 5) {
      struct profile *p = &s.profiles[s.np];
      if (sscanf(line, "P\t%llu\t%d\t%192[^\n]", &p->id, &p->active, p->name) == 3 && p->id)
        s.np++;
    }
    else if (line[0] == 'D'  && s.n < MAX_ROWS) {
      struct row *r = &s.rows[s.n];
      char *sv, *p = strtok_r(line, "\t", &sv);
      int i = 0;
      while ((p = strtok_r(NULL, "\t", &sv))) {
        switch (i++) {
        case 0:
          snprintf(r->mac, sizeof(r->mac), "%s", p);
          break;
        case 1:
          r->selected = atoi(p) != 0;
          break;
        case 2:
          r->online = atoi(p) != 0;
          break;
        case 3:
          snprintf(r->ip, sizeof(r->ip), "%s", p);
          break;
        case 4:
          snprintf(r->name, sizeof(r->name), "%s", p);
          break;
        }
      }
      if (i == 5 && strlen(r->mac) == 17)
        s.n++;
    }
    line = strtok_r(NULL, "\n", &save);
  }
  if (valid)
    *out = s;
  return valid;
}
#ifndef SCREEN_PREVIEW
static int exchange(const char *request, char *reply, size_t cap) {
  int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (fd < 0)
    return -1;
  struct timeval t = {.tv_sec = 0, .tv_usec = 300000};
  setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &t, sizeof(t));
  setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &t, sizeof(t));
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  strcpy(a.sun_path, RUNDIR "/control.sock");
  int ok = -1;
  if (connect(fd, (void *)&a, sizeof(a)) == 0 &&
      send(fd, request, strlen(request), MSG_NOSIGNAL) ==
          (ssize_t)strlen(request)) {
    size_t n = 0;
    ssize_t k;
    while (n + 1 < cap && (k = read(fd, reply + n, cap - n - 1)) > 0)
      n += (size_t)k;
    reply[n] = 0;
    if (n > 0)
      ok = 0;
  }
  close(fd);
  return ok;
}
#else
static char test_request[4096];
static int exchange(const char *r, char *out, size_t cap) {
  snprintf(test_request, sizeof(test_request), "%s", r);
  snprintf(out, cap, "OK 1\n");
  return 0;
}
#endif
static void submit(int toggle, int value) {
  char req[4096], reply[128];
  if (blocked())
    return;
  if (toggle == 2)
    snprintf(req, sizeof(req), "profile %llu %llu\n", chosen_revision, chosen_profile);
  else if (toggle)
    snprintf(req, sizeof(req), "toggle %llu %d\n", data.rev, value);
  else {
    size_t n = (size_t)snprintf(req, sizeof(req), "devices %llu ", draft.rev);
    int count = 0;
    for (int i = 0; i < draft.n; i++) {
      // Send all draft entries on version conflict; agent rejects the stale
      // revision atomically.
      n += (size_t)snprintf(req + n, sizeof(req) - n, "%s%s=%d",
                            count++ ? "," : "", draft.rows[i].mac,
                            draft.rows[i].selected);
    }
    if (!count) {
      dirty = 0;
      page = WG;
      return;
    }
    snprintf(req + n, sizeof(req) - n, "\n");
  }
  pending_revision = toggle == 2 ? chosen_revision : toggle ? data.rev : draft.rev;
  if (exchange(req, reply, sizeof(reply)) == 0 &&
      sscanf(reply, "OK %llu", &pending) == 1) {
    dialog = 0;
    notice[0] = 0;
  } else {
    strcpy(notice, "Request failed. Try again.");
    dialog = 3;
    pending = 0;
  }
  need_draw = 1;
}
#include "devui/actions.h"
#ifndef SCREEN_PREVIEW
static void putfile(const char *path, const char *s) {
  char tmp[256];
  snprintf(tmp, sizeof(tmp), "%s.new", path);
  int fd = open(tmp, O_CREAT | O_WRONLY | O_TRUNC | O_CLOEXEC, 0600);
  if (fd >= 0) {
    if (write(fd, s, strlen(s)) == (ssize_t)strlen(s)) {
      fsync(fd);
      rename(tmp, path);
    }
    close(fd);
  }
}
static int readint(const char *path) {
  FILE *f = fopen(path, "r");
  int v = -1;
  if (f) {
    if (fscanf(f, "%d", &v) != 1)
      v = -1;
    fclose(f);
  }
  return v;
}
static void brightness(int v) {
  int fd = open(BL, O_WRONLY | O_CLOEXEC);
  if (fd >= 0) {
    char b[16];
    int n = snprintf(b, sizeof(b), "%d", v);
    (void)!write(fd, b, (size_t)n);
    close(fd);
  }
}
/* Observe the dedicated key device without grabbing it: factory long-press
 * handling remains available. Never open the touchscreen as the power key. */
static int power_open(void) {
  for (int i = 0; i < 32; i++) {
    char path[64], name[128] = {0};
    snprintf(path, sizeof(path), "/dev/input/event%d", i);
    int fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) continue;
    unsigned char keys[(KEY_MAX + 8) / 8] = {0};
    if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) >= 0 &&
        !strcmp(name, "pmic_pwrkey") &&
        ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(keys)), keys) >= 0 &&
        (keys[KEY_POWER / 8] & (1u << (KEY_POWER % 8)))) {
      return fd;
    }
    close(fd);
  }
  return -1;
}
static int power_check;
static int screen_toggle(int *on_level) {
  int current = readint(BL);
  if (screen_awake && current > 0) *on_level = current;
  int target = screen_awake ? 0 : *on_level;
  brightness(target);
  if (readint(BL) != target) {
    logline("power key: backlight write failed");
    return -1;
  }
  screen_awake = !screen_awake;
  need_draw = 1;
  logline("power key: display %s", screen_awake ? "awake" : "asleep");
  return 0;
}
static int processes(const char *name, int sig) {
  DIR *dir = opendir("/proc");
  if (!dir)
    return 0;
  struct dirent *e;
  int count = 0;
  while ((e = readdir(dir))) {
    int pid = atoi(e->d_name);
    if (pid <= 1)
      continue;
    char p[128], exe[256];
    snprintf(p, sizeof(p), "/proc/%d/exe", pid);
    ssize_t n = readlink(p, exe, sizeof(exe) - 1);
    if (n < 0)
      continue;
    exe[n] = 0;
    char *base = strrchr(exe, '/');
    if (base && !strcmp(base + 1, name)) {
      count++;
      if (sig)
        kill(pid, sig);
    }
  }
  closedir(dir);
  return count;
}
static int run(char *const args[], char *out, size_t cap) {
  int fds[2];
  if (pipe2(fds, O_CLOEXEC | O_NONBLOCK) < 0)
    return -1;
  pid_t pid = fork();
  if (pid < 0) {
    close(fds[0]);
    close(fds[1]);
    return -1;
  }
  if (!pid) {
    setpgid(0, 0);
    dup2(fds[1], 1);
    int n = open("/dev/null", O_RDWR);
    if (n >= 0) {
      dup2(n, 0);
      dup2(n, 2);
    }
    close(fds[0]);
    close(fds[1]);
    execvp(args[0], args);
    _exit(127);
  }
  close(fds[1]);
  setpgid(pid, pid);
  long long end = monotonic_ms() + 6000;
  int status = 0, done = 0;
  size_t used = 0;
  while (monotonic_ms() < end) {
    char buf[512];
    ssize_t n = read(fds[0], buf, sizeof(buf));
    if (n > 0 && out && used + 1 < cap) {
      size_t copy = (size_t)n < cap - used - 1 ? (size_t)n : cap - used - 1;
      memcpy(out + used, buf, copy);
      used += copy;
    }
    if (waitpid(pid, &status, WNOHANG) == pid) {
      done = 1;
      break;
    }
    usleep(20000);
  }
  if (!done) {
    kill(-pid, SIGKILL);
    waitpid(pid, &status, 0);
  }
  if (out && cap) {
    ssize_t n;
    while (used + 1 < cap && (n = read(fds[0], out + used, cap - used - 1)) > 0)
      used += (size_t)n;
  }
  if (out && cap)
    out[used] = 0;
  close(fds[0]);
  return done && WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}
static int restore(void) {
  putfile(RUNDIR "/state", "restoring");
  int bl = readint(RUNDIR "/brightness"), touch = readint(RUNDIR "/touch"),
      ui = readint(RUNDIR "/ui");
  int ok = 1;
  if (ui == 1 && !processes("zte_topsw_devui", 0)) {
    char *args[] = {"/etc/init.d/zte_topsw_devui", "start", NULL};
    if (run(args, NULL, 0))
      ok = 0;
    for (int i = 0; i < 30 && !processes("zte_topsw_devui", 0); i++)
      usleep(100000);
    if (!processes("zte_topsw_devui", 0))
      ok = 0;
  }
  if (touch == 1 && !processes("mtdev2tuio", 0)) {
    pid_t p = fork();
    if (!p) {
      setsid();
      int n = open("/dev/null", O_RDWR);
      if (n >= 0) {
        dup2(n, 0);
        dup2(n, 1);
        dup2(n, 2);
      }
      execlp("mtdev2tuio", "mtdev2tuio", "/dev/input/event3",
             "osc.udp://127.0.0.1:3333/", (char *)NULL);
      _exit(127);
    }
    usleep(300000);
    if (!processes("mtdev2tuio", 0))
      ok = 0;
  }
  if (bl >= 0 && bl <= 255)
    brightness(bl);
  // Stock UI resumes its own sleep/brightness policy after ownership is
  // returned.
  char restored[16];
  snprintf(restored, sizeof(restored), "%d", readint(BL));
  putfile(RUNDIR "/brightness-restored", restored);
  putfile(RUNDIR "/state", ok ? "idle" : "error");
  return ok ? 0 : 1;
}
struct input_touch {
  int fd, slot, id[2], x, y, xok, yok;
  struct input_absinfo ax, ay;
  int down, sx, sy, moved, multi, dropped, suppress;
};
static int input_open(struct input_touch *t) {
  memset(t, 0, sizeof(*t));
  t->fd = -1;
  t->id[0] = t->id[1] = -1;
  for (int i = 0; i < 16; i++) {
    char p[64], name[128] = {0};
    snprintf(p, sizeof(p), "/dev/input/event%d", i);
    int fd = open(p, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0)
      continue;
    if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) >= 0 &&
        strstr(name, "sitronix") &&
        ioctl(fd, EVIOCGABS(ABS_MT_POSITION_X), &t->ax) == 0 &&
        ioctl(fd, EVIOCGABS(ABS_MT_POSITION_Y), &t->ay) == 0) {
      t->fd = fd;
      break;
    }
    close(fd);
  }
  return t->fd >= 0 && t->ax.maximum > t->ax.minimum &&
                 t->ay.maximum > t->ay.minimum
             ? 0
             : -1;
}
static int axis(int v, struct input_absinfo *a, int size) {
  long n = (long)(v - a->minimum) * (size - 1) / (a->maximum - a->minimum);
  return n < 0 ? 0 : n >= size ? size - 1 : (int)n;
}
static int input_event(struct input_touch *t, struct input_event *e) {
  if (e->type == EV_SYN && e->code == SYN_DROPPED) {
    t->dropped = 1;
    t->down = 0;
    t->id[0] = t->id[1] = -1;
    return 0;
  }
  if (t->dropped) {
    if (e->type == EV_SYN && e->code == SYN_REPORT) {
      t->dropped = 0;
      t->xok = t->yok = 0;
    }
    return 0;
  }
  if (e->type == EV_ABS) {
    if (e->code == ABS_MT_SLOT)
      t->slot = e->value;
    else if (t->slot >= 0 && t->slot < 2) {
      if (e->code == ABS_MT_TRACKING_ID) {
        t->id[t->slot] = e->value;
        if (e->value >= 0 && t->slot == 0)
          t->xok = t->yok = 0;
      } else if (t->slot == 0 && e->code == ABS_MT_POSITION_X) {
        t->x = e->value;
        t->xok = 1;
      } else if (t->slot == 0 && e->code == ABS_MT_POSITION_Y) {
        t->y = e->value;
        t->yok = 1;
      }
    }
  }
  if (e->type == EV_KEY && e->code == BTN_TOUCH && e->value == 0)
    t->id[0] = t->id[1] = -1;
  if (e->type != EV_SYN || e->code != SYN_REPORT)
    return 0;
  int x = axis(t->x, &t->ax, W), y = axis(t->y, &t->ay, H);
  rotate_point(&x, &y);
  if (t->id[1] >= 0)
    t->multi = 1;
  if (t->id[0] >= 0 && t->xok && t->yok) {
    if (!t->down) {
      t->down = 1;
      t->sx = x;
      t->sy = y;
      t->moved = 0;
      t->multi = t->id[1] >= 0;
    }
    if (!screen_awake || t->suppress) t->multi = 1;
    if (abs(x - t->sx) > 12 || abs(y - t->sy) > 12)
      t->moved = 1;
  } else if (t->down && t->id[0] < 0) {
    if (screen_awake && !t->suppress && !t->multi && !t->moved)
      tap(t->sx, t->sy);
    t->down = 0;
    return 1;
  }
  return t->id[0] >= 0;
}
static int input_self_test(void) {
  power_self_test();
  struct drm_buf frame = {.pitch = W * 2, .map = calloc(W * H, 2)};
  assert(frame.map);
  page = MENU;
  render(&frame);
  int x = 0, y = 0;
  rotate_point(&x, &y);
  assert(x == 319 && y == 479);
  rotate_point(&x, &y);
  assert(x == 0 && y == 0);
  struct input_touch t = {.fd = -1,
                          .id = {-1, -1},
                          .ax = {.minimum = 0, .maximum = 319},
                          .ay = {.minimum = 0, .maximum = 479}};
  const struct input_event events[] = {
      {.type = EV_ABS, .code = ABS_MT_SLOT, .value = 0},
      {.type = EV_ABS, .code = ABS_MT_TRACKING_ID, .value = 1},
      {.type = EV_ABS, .code = ABS_MT_POSITION_X, .value = 69},
      {.type = EV_ABS, .code = ABS_MT_POSITION_Y, .value = 459},
      {.type = EV_SYN, .code = SYN_REPORT},
      {.type = EV_ABS, .code = ABS_MT_TRACKING_ID, .value = -1},
      {.type = EV_SYN, .code = SYN_REPORT}};
  for (size_t i = 0; i < sizeof(events) / sizeof(events[0]); i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(quitting == 1);
  quitting = 0;
  screen_awake = 0;
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  screen_awake = 1; /* A finger held over wake must not activate Stock UI. */
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  assert(!quitting);
  t.suppress = 1; /* Entire dark tap queued before the wake key was drained. */
  for (size_t i = 0; i < 7; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  assert(!quitting);
  t.suppress = 0;
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  struct input_event move = {
      .type = EV_ABS, .code = ABS_MT_POSITION_X, .value = 109};
  input_event(&t, &move);
  struct input_event syn = {.type = EV_SYN, .code = SYN_REPORT};
  input_event(&t, &syn);
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(!quitting);
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  struct input_event drop = {.type = EV_SYN, .code = SYN_DROPPED};
  input_event(&t, &drop);
  input_event(&t, &syn);
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(!quitting);
  free(frame.map);
  puts("Power press/repeat/long/drop, dark-touch rejection and rotated MT-B tests passed");
  return 0;
}
static int panel(int heartbeat, int idle_seconds) {
  page = MENU;
  struct qpic_ctx drm;
  struct input_touch input;
  int result = 1;
  if (input_open(&input) < 0) {
    logline("touch open failed: %s", strerror(errno));
    return 1;
  }
  if (drm_init(&drm) < 0) {
    drm_destroy(&drm);
    close(input.fd);
    return 1;
  }
  int keyfd = power_open();
  if (keyfd < 0) {
    logline("power key open failed");
    drm_destroy(&drm); close(input.fd); return 1;
  }
  struct power_button key = {0};
  int on_level = readint(BL), check_step = 0;
  if (on_level <= 0) on_level = 128;
  screen_awake = 1;
  long long started = monotonic_ms();
  long long last = monotonic_ms(), next = 0, beat = 0, contact = last;
  int first = 1;
  char reply[49152];
  while (!g_stop && !quitting &&
         monotonic_ms() - last < (long long)idle_seconds * 1000) {
    long long now = monotonic_ms();
    if (now >= next) {
      next = now + 2000;
      struct snapshot incoming;
      if (exchange("status\n", reply, sizeof(reply)) == 0) {
        contact = now;
        if (parse_snapshot(reply, &incoming)) {
          data = incoming;
          need_draw = 1;
          if (pending && data.done >= pending) {
            if (data.result) {
              pending = 0;
              strcpy(notice, data.result == 1 ? "Settings changed. Refresh."
                                              : "Apply failed. Check OpenUI.");
              dialog = 3;
            } else if (data.rev > pending_revision) {
              pending = 0;
              dirty = 0;
              notice[0] = 0;
              if (page == DEVICES) {
                draft = data;
                for (int i = 0; i < draft.n; i++)
                  original_selection[i] = draft.rows[i].selected;
                if (return_after_save)
                  page = WG;
              }
              return_after_save = 0;
            }
          }
        }
      }
    }
    if (now - contact > 15000)
      break;
    if (need_draw && screen_awake) {
      struct drm_buf *frame = drm_draw_buf(&drm);
      render(frame);
      rotate_frame(frame);
      if (first || memcmp(frame->map, drm.bufs[drm.cur_buf ^ 1].map,
                          (size_t)frame->pitch * H)) {
        if (drm_flip(&drm) < 0)
          goto end;
      }
      need_draw = 0;
      if (first) {
        (void)!write(heartbeat, "R", 1);
        first = 0;
      }
    }
    if (now >= beat) {
      (void)!write(heartbeat, "H", 1);
      beat = now + 1000;
    }
    if (getppid() == 1)
      break;
    if (power_check && check_step < 3 && now - started >= (check_step + 1) * 1000) {
      if (screen_toggle(&on_level)) goto end;
      input.multi = 1;
      check_step++;
    }
    struct pollfd p[2] = {{.fd = input.fd, .events = POLLIN},
                         {.fd = keyfd, .events = POLLIN}};
    if (poll(p, 2, 50) < 0 && errno != EINTR) goto end;
    if ((p[0].revents | p[1].revents) & (POLLERR | POLLHUP | POLLNVAL)) goto end;
    struct input_event ev;
    input.suppress = 0;
    while (read(keyfd, &ev, sizeof(ev)) == sizeof(ev)) {
      long long stamp = (long long)ev.time.tv_sec * 1000 + ev.time.tv_usec / 1000;
      if (power_event(&key, ev.type, ev.code, ev.value, stamp)) {
        if (screen_toggle(&on_level)) goto end;
        input.multi = 1;
        input.suppress = 1; /* Reject touch queued across a backlight transition. */
        last = monotonic_ms();
      }
    }
    while (read(input.fd, &ev, sizeof(ev)) == sizeof(ev)) {
      if (input_event(&input, &ev) && screen_awake)
        last = monotonic_ms();
    }
  }
  result = power_check && check_step != 3 ? 1 : 0;
end:
  close(keyfd);
  drm_destroy(&drm);
  close(input.fd);
  return result;
}
static int supervise(int seconds, int recovery) {
  umask(077);
  if (mkdir(RUNDIR, 0700) < 0 && errno != EEXIST)
    return 1;
  int lock = open(RUNDIR "/lock", O_CREAT | O_RDWR | O_CLOEXEC, 0600);
  if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB) < 0)
    return 2;
  char buf[2048];
  snprintf(buf, sizeof(buf), "%d", getpid());
  putfile(RUNDIR "/owner", buf);
  signal(SIGINT, on_sig);
  signal(SIGTERM, on_sig);
  signal(SIGHUP, SIG_IGN);
  signal(SIGPIPE, SIG_IGN);
  if (recovery) {
    int rc = restore();
    unlink(RUNDIR "/owner");
    close(lock);
    return rc;
  }
  char *sync[] = {"ubus",          "call", "zwrt_topsw_daemon.sync",
                  "get_sync_info", "{}",   NULL};
  int bl = readint(BL), ui = processes("zte_topsw_devui", 0) > 0,
      touch = processes("mtdev2tuio", 0) > 0;
  if (run(sync, buf, sizeof(buf)) || !strstr(buf, "sync success") ||
      !strstr(buf, "register success") || !ui || bl < 0) {
    putfile(RUNDIR "/state", "error");
    unlink(RUNDIR "/owner");
    close(lock);
    return 1;
  }
  snprintf(buf, sizeof(buf), "%d", bl);
  putfile(RUNDIR "/brightness", buf);
  putfile(RUNDIR "/ui", ui ? "1" : "0");
  putfile(RUNDIR "/touch", touch ? "1" : "0");
  putfile(RUNDIR "/state", "starting");
  char *stop[] = {"/etc/init.d/zte_topsw_devui", "stop", NULL};
  int rc = 1;
  int stop_result = run(stop, NULL, 0);
  logline("stock stop result=%d", stop_result);
  if (stop_result)
    goto done;
  // Some firmware starts devui outside procd; stop only matching executable
  // paths.
  processes("zte_topsw_devui", SIGTERM);
  if (touch)
    processes("mtdev2tuio", SIGTERM);
  for (int i = 0; i < 30 && (processes("zte_topsw_devui", 0) ||
                             processes("mtdev2tuio", 0));
       i++)
    usleep(100000);
  processes("zte_topsw_devui", SIGKILL);
  if (touch)
    processes("mtdev2tuio", SIGKILL);
  usleep(200000);
  logline("after stop: ui=%d touch=%d signal=%d",
          processes("zte_topsw_devui", 0), processes("mtdev2tuio", 0),
          (int)g_stop);
  if (processes("zte_topsw_devui", 0) || processes("mtdev2tuio", 0) || g_stop)
    goto done;
  brightness(bl >= 64 ? bl : 128);
  int pipefd[2];
  if (pipe2(pipefd, O_CLOEXEC | O_NONBLOCK) < 0)
    goto done;
  pid_t child = fork();
  if (child < 0) {
    close(pipefd[0]);
    close(pipefd[1]);
    goto done;
  }
  if (!child) {
    close(pipefd[0]);
    close(lock);
    int r = panel(pipefd[1], seconds);
    close(pipefd[1]);
    if (getppid() == 1)
      restore();
    _exit(r);
  }
  close(pipefd[1]);
  snprintf(buf, sizeof(buf), "%d", child);
  putfile(RUNDIR "/child", buf);
  long long beat = monotonic_ms();
  int status = 0, exited = 0, ready = 0;
  while (!g_stop && monotonic_ms() - beat < 10000) {
    char bytes[64];
    ssize_t n = read(pipefd[0], bytes, sizeof(bytes));
    if (n > 0) {
      beat = monotonic_ms();
      for (ssize_t i = 0; i < n; i++)
        if (bytes[i] == 'R') {
          ready = 1;
          putfile(RUNDIR "/state", "active");
        }
    }
    if (waitpid(child, &status, WNOHANG) == child) {
      exited = 1;
      break;
    }
    usleep(100000);
  }
  if (!exited) {
    kill(child, SIGTERM);
    for (int i = 0; i < 20; i++) {
      if (waitpid(child, &status, WNOHANG) == child) {
        exited = 1;
        break;
      }
      usleep(100000);
    }
    if (!exited) {
      kill(child, SIGKILL);
      waitpid(child, &status, 0);
    }
  }
  rc = ready && WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : 1;
  close(pipefd[0]);
  unlink(RUNDIR "/child");
done:;
  int restored = restore();
  logline("restoration result=%d; panel result=%d", restored, rc);
  if (restored)
    rc = 1;
  if (rc)
    putfile(RUNDIR "/state", "error");
  unlink(RUNDIR "/owner");
  close(lock);
  return rc;
}
int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--self-test"))
    return input_self_test();
  if (argc == 2 && !strcmp(argv[1], "--version")) {
    puts("openui-screen 3 DevUI / Atomic / OpenUI agent / power key");
    return 0;
  }
  if (argc == 2 && !strcmp(argv[1], "--ipc-check")) {
    // Read-only integration probe; never prints names, addresses or credentials.
    char reply[49152];struct snapshot snap;
    for(int i=0;i<8;i++) {
      if(exchange("status\n",reply,sizeof(reply))==0&&parse_snapshot(reply,&snap)&&snap.stamp) {
        printf("Agent IPC: %d profiles, %d devices, overview=%s\n",snap.np,snap.n,snap.uptime?"ready":"unavailable");
        return snap.uptime?0:1;
      }
      usleep(500000);
    }
    return 1;
  }
  if (argc == 2 && !strcmp(argv[1], "--power-check")) {
    power_check = 1;
    return supervise(5, 0);
  }
  if (argc == 2 && !strcmp(argv[1], "--probe")) {
    int fd = power_open();
    if (fd < 0) return 1;
    close(fd);
    puts("Power: pmic_pwrkey KEY_POWER, non-exclusive observer");
    struct input_touch t;
    if (input_open(&t) < 0)
      return 1;
    printf("Touch X=%d..%d Y=%d..%d; target 320x480 RGB565 Atomic\n",
           t.ax.minimum, t.ax.maximum, t.ay.minimum, t.ay.maximum);
    close(t.fd);
    return 0;
  }
  if (argc >= 2 && !strcmp(argv[1], "--supervise")) {
    int seconds = argc == 3 ? atoi(argv[2]) : 120;
    if (seconds < 3 || seconds > 120)
      return 2;
    return supervise(seconds, 0);
  }
  if (argc == 2 && !strcmp(argv[1], "--restore"))
    return supervise(0, 1);
  fprintf(stderr, "Use --supervise [3..120 idle seconds] or --restore\n");
  return 2;
}
#else
static void dump(struct drm_buf *b, const char *path) {
  FILE *f = fopen(path, "wb");
  assert(f);
  fprintf(f, "P6\n320 480\n255\n");
  for (int i = 0; i < W * H; i++) {
    uint16_t c = b->map[i];
    unsigned char rgb[3] = {(unsigned char)(((c >> 11) & 31) * 255 / 31),
                            (unsigned char)(((c >> 5) & 63) * 255 / 63),
                            (unsigned char)((c & 31) * 255 / 31)};
    fwrite(rgb, 1, 3, f);
  }
  fclose(f);
}
static void capture(struct drm_buf *b,const char *dir,const char *name) {
  char path[512];render(b);snprintf(path,sizeof(path),"%s/%s.ppm",dir,name);dump(b,path);
}
static void press_id(struct drm_buf *b,const char *id) {
  render(b);
  char selector[48];snprintf(selector,sizeof(selector),"#%s",id);
  int x,y,w,h;
  assert(html_view_rect(selector,&x,&y,&w,&h));
  assert(w>=48&&h>=48&&x>=0&&y>=0&&x+w<=W&&y+h<=H);
  tap(x+w/2,y+h/2);
}
int main(int argc, char **argv) {
  power_self_test();
  if(argc<2)return 2;
  struct drm_buf b={.pitch=W*2,.map=calloc(W*H,2)};
  assert(b.map);
  struct drm_buf padded={.pitch=(W+3)*2,.map=calloc((W+3)*H,2)};
  assert(padded.map);padded.map[0]=1;padded.map[W-1]=2;
  padded.map[(W+3)*(H-1)]=3;padded.map[(W+3)*(H-1)+W-1]=4;padded.map[W]=0x1234;
  rotate_frame(&padded);
  assert(padded.map[0]==4&&padded.map[W-1]==3&&padded.map[W]==0x1234);
  rotate_frame(&padded);assert(padded.map[0]==1);free(padded.map);
  char example[4096];
  snprintf(example,sizeof(example),
    "S 7 1 1 1 0 %llu 134217728 25165824 1 %llu\n"
    "D\t02:11:22:33:44:55\t1\t1\t192.0.2.2\tWork laptop\n"
    "D\t02:11:22:33:44:56\t0\t1\t192.0.2.3\tiPhone\n"
    "D\t02:11:22:33:44:57\t1\t0\t-\t-\n"
    "D\t02:11:22:33:44:58\t0\t0\t-\tWork laptop\n"
    "D\t02:11:22:33:44:59\t0\t0\t-\t<a href='act:stock'> A very long device name\n"
    "P\t1\t1\tHome VPN\nP\t2\t0\tTravel VPN\nP\t3\t0\tWork VPN\n"
    "P\t4\t0\tBackup VPN\nP\t5\t0\tA long profile name <a href='act:stock'>\n"
    "O 83 320 18372 34.5 12.4\nJ 0 0 0 0\n.\n",
    (unsigned long long)time(NULL)-15,(unsigned long long)time(NULL));
  assert(parse_snapshot(example,&data));assert(data.n==5&&data.np==5&&data.battery==83);
  capture(&b,argv[1],"menu");press_id(&b,"overview");assert(page==OVERVIEW);
  capture(&b,argv[1],"overview");press_id(&b,"back");assert(page==MENU);
  press_id(&b,"wireguard");assert(page==WG);capture(&b,argv[1],"home");
  rotate_frame(&b);char path[512];snprintf(path,sizeof(path),"%s/home-panel.ppm",argv[1]);dump(&b,path);
  press_id(&b,"devices");assert(page==DEVICES);press_id(&b,"d1");assert(dirty&&draft.rows[1].selected);
  capture(&b,argv[1],"devices");press_id(&b,"next");assert(offset==4);
  capture(&b,argv[1],"devices-page2");assert(strstr(html,"&lt;a href="));
  press_id(&b,"prev");assert(offset==0);press_id(&b,"back");assert(dialog==2);
  capture(&b,argv[1],"unsaved");press_id(&b,"saveback");assert(strstr(test_request,"devices 7 ")&&pending==1);
  pending=0;dirty=0;dialog=0;page=WG;
  press_id(&b,"toggle");assert(dialog==1);capture(&b,argv[1],"confirm");press_id(&b,"confirm");
  assert(!strcmp(test_request,"toggle 7 0\n"));pending=0;dialog=0;
  press_id(&b,"profiles");assert(page==PROFILES);capture(&b,argv[1],"profiles");
  press_id(&b,"p4");assert(chosen_profile==5&&chosen_revision==7&&dialog==4);
  capture(&b,argv[1],"profile-confirm");data.rev=8;
  press_id(&b,"confirm");assert(!strcmp(test_request,"profile 7 5\n"));
  pending=0;dialog=3;strcpy(notice,"Settings changed. Refresh.");capture(&b,argv[1],"conflict");
  press_id(&b,"refresh");assert(page==WG&&!dirty&&!dialog);
  data.has=0;data.enabled=0;data.connected=0;capture(&b,argv[1],"unconfigured");
  test_request[0]=0;press_id(&b,"toggle");assert(!test_request[0]);
  data.has=1;capture(&b,argv[1],"off");
  data.enabled=1;data.handshake=0;capture(&b,argv[1],"waiting");
  data.error=1;capture(&b,argv[1],"error");data.error=0;
  data.stamp=(unsigned long long)time(NULL)-20;capture(&b,argv[1],"stale");
  press_id(&b,"toggle");assert(!test_request[0]);
  page=MENU;press_id(&b,"stock");assert(quitting);
  free(b.map);puts("PASS DevUI HTML hit testing, menu navigation, profile version capture, device pagination/drafts, escaping, confirmations, disabled states and rotation");
  return 0;
}
#endif

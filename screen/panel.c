/* SPDX-License-Identifier: GPL-3.0-or-later
 * WireGuard touch UI; native display and root IPC only. See LICENSE and NOTICE.
 */
#define _GNU_SOURCE
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
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
#include "font.h"
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
struct snapshot {
  unsigned long long rev, handshake, rx, tx, stamp, job, done;
  int has, enabled, connected, error, all, busy, result, n;
  struct row rows[MAX_ROWS];
};
static struct snapshot data, draft;
static int original_selection[MAX_ROWS];
static int page, offset, dialog, dirty, quitting, need_draw = 1,
                                                  return_after_save;
static unsigned long long pending, pending_revision;
static char notice[128];
static long long monotonic_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (long long)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}
static unsigned codepoint(const char **p) {
  const unsigned char *s = (const unsigned char *)*p;
  unsigned c = *s++;
  int n = 0;
  if (c >= 0xf0 && c <= 0xf4) {
    c &= 7;
    n = 3;
  } else if (c >= 0xe0 && c <= 0xef) {
    c &= 15;
    n = 2;
  } else if (c >= 0xc2 && c <= 0xdf) {
    c &= 31;
    n = 1;
  } else if (c >= 128)
    c = '?';
  for (int i = 0; i < n; i++) {
    if ((*s & 0xc0) != 0x80) {
      c = '?';
      break;
    }
    c = (c << 6) | (*s++ & 63);
  }
  *p = (const char *)s;
  return c < 65536 ? c : '?';
}
static int text_width(const char *s, int size) {
  int x = 0;
  while (*s) {
    unsigned c = codepoint(&s);
    x += (font[c][0] ? font[c][0] : 8) * size / 16 + 1;
  }
  return x;
}
static void text(struct drm_buf *b, int x, int y, const char *s, int size,
                 uint16_t col, int maxw) {
  int start = x;
  while (*s) {
    unsigned c = codepoint(&s);
    if (!font[c][0])
      c = '?';
    int w = font[c][0], dw = w * size / 16;
    if (x + dw > start + maxw)
      break;
    for (int j = 0; j < size; j++)
      for (int i = 0; i < dw; i++) {
        int sy = j * 16 / size, sx = i * 16 / size;
        unsigned v = font[c][1 + sy * (w / 8) + sx / 8];
        if (v & (128 >> (sx % 8)))
          draw_pixel(b, x + i, y + j, col);
      }
    x += dw + 1;
  }
}
static void button(struct drm_buf *b, int x, int y, int w, int h, const char *s,
                   uint16_t col) {
  fill_rect(b, x, y, x + w, y + h, col);
  int tw = text_width(s, 20);
  text(b, x + (w - tw) / 2, y + (h - 20) / 2, s, 20, FG, w - 8);
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
static void render(struct drm_buf *b) {
  char tmp[128], rx[32], tx[32];
  fill(b, BG);
  text(b, 16, 18, page ? "Devices" : "WireGuard", 22, FG, 185);
  button(b, 210, 4, 106, 48, page ? "Back" : "Stock UI", CARD);
  if (!page) {
    fill_rect(b, 12, 68, 308, 194, CARD);
    const char *status = !data.stamp            ? "Loading status"
                         : pending || data.busy ? "Applying changes"
                         : stale()              ? "Status is stale"
                         : !data.has            ? "No configuration"
                         : data.error           ? "Tunnel error"
                         : !data.enabled        ? "Off"
                         : data.connected       ? "Handshake recent"
                                                : "Awaiting handshake";
    text(b, 26, 86, status, 26,
         data.error || stale() ? RED
         : data.connected      ? GREEN
                               : FG,
         276);
    if (!data.has)
      strcpy(tmp, "Import a config in OpenUI");
    else if (!data.enabled)
      strcpy(tmp, "Devices use normal internet");
    else if (data.handshake)
      snprintf(tmp, sizeof(tmp), "Last handshake: %llu s ago",
               (unsigned long long)time(NULL) > data.handshake
                   ? (unsigned long long)time(NULL) - data.handshake
                   : 0);
    else
      strcpy(tmp, "Waiting for the server");
    text(b, 26, 138, tmp, 16, MUTED, 270);
    text(b, 26, 164, "Internet access not verified", 16, MUTED, 270);
    text(b, 18, 210, "Downloaded", 16, MUTED, 140);
    text(b, 174, 210, "Uploaded", 16, MUTED, 135);
    amount(rx, sizeof(rx), data.rx);
    amount(tx, sizeof(tx), data.tx);
    text(b, 18, 234, rx, 24, FG, 145);
    text(b, 174, 234, tx, 24, FG, 140);
    button(b, 12, 282, 296, 56,
           data.enabled ? "Disable WireGuard" : "Enable WireGuard",
           blocked() || !data.has ? CARD : BLUE);
    int selected = 0;
    for (int i = 0; i < data.n; i++)
      selected += data.rows[i].selected;
    snprintf(tmp, sizeof(tmp), "Devices   %d / %d >", selected, data.n);
    button(b, 12, 354, 296, 56, tmp, CARD);
    text(b, 16, 428,
         data.all ? "New devices: WireGuard" : "New devices: normal network",
         16, MUTED, 294);
    text(b, 16, 452, notice[0] ? notice : "Returns to stock UI after 2 min", 16,
         notice[0] ? RED : MUTED, 294);
  } else {
    text(b, 16, 62,
         draft.all ? "New devices: WireGuard" : "New devices: normal network",
         16, MUTED, 290);
    text(b, 16, 82,
         data.enabled ? "Checked: WG | unchecked: direct"
                      : "Active when tunnel is enabled",
         16, MUTED, 290);
    for (int i = 0; i < 4 && offset + i < draft.n; i++) {
      struct row *r = &draft.rows[offset + i];
      int y = 106 + i * 60;
      fill_rect(b, 12, y, 308, y + 56, CARD);
      text(b, 20, y + 17, r->selected ? "[x]" : "[ ]", 20,
           r->selected ? GREEN : MUTED, 38);
      int duplicate = 0;
      for (int j = 0; j < draft.n; j++)
        if (j != offset + i && !strcmp(r->name, draft.rows[j].name))
          duplicate = 1;
      const char *name = strcmp(r->name, "-") ? r->name : "Unknown device";
      text(b, 62, y + 5, name, 18, FG,
           duplicate || !strcmp(r->name, "-") ? 153 : 235);
      if (duplicate || !strcmp(r->name, "-"))
        text(b, 224, y + 6, r->mac + 9, 14, MUTED, 78);
      snprintf(tmp, sizeof(tmp), "%s  %s", r->online ? "Online" : "Offline",
               r->ip);
      text(b, 62, y + 32, tmp, 16, MUTED, 235);
    }
    if (!draft.n)
      text(b, 38, 196, "No devices found", 18, MUTED, 250);
    button(b, 12, 354, 96, 48, "Previous", offset ? BLUE : CARD);
    button(b, 212, 354, 96, 48, "Next", offset + 4 < draft.n ? BLUE : CARD);
    snprintf(tmp, sizeof(tmp), "%d / %d", offset / 4 + 1,
             (draft.n + 3) / 4 ? (draft.n + 3) / 4 : 1);
    text(b, 125, 369, tmp, 18, MUTED, 80);
    button(b, 12, 416, 142, 52, "Cancel", CARD);
    button(b, 166, 416, 142, 52, pending ? "Applying" : "Save",
           dirty && !blocked() ? BLUE : CARD);
  }
  if (dialog) {
    fill_rect(b, 0, 56, W, H, BG);
    fill_rect(b, 12, 108, 308, 466, CARD);
    if (dialog == 1) {
      text(b, 30, 135, "Disable WireGuard?", 24, FG, 270);
      text(b, 30, 190, "Devices will return to", 20, MUTED, 265);
      text(b, 30, 220, "the normal network.", 20, MUTED, 260);
      button(b, 24, 300, 128, 56, "Cancel", BG);
      button(b, 168, 300, 128, 56, "Disable", BLUE);
    } else if (dialog == 2) {
      text(b, 30, 132, "Unsaved changes", 24, FG, 265);
      button(b, 24, 210, 272, 56, "Save and return", BLUE);
      button(b, 24, 282, 272, 56, "Discard changes", BG);
      button(b, 24, 354, 272, 56, "Keep editing", BG);
    } else {
      text(b, 26, 136, "Action failed", 24, RED, 275);
      text(b, 26, 190, notice, 16, FG, 275);
      text(b, 26, 222, "Refresh or check OpenUI.", 16, MUTED, 275);
      button(b, 24, 300, 272, 56, "Refresh and back", BLUE);
    }
  }
}
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
  struct snapshot s = {0};
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
    else if (line[0] == 'D' && s.n < MAX_ROWS) {
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
  if (toggle)
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
      page = 0;
      return;
    }
    snprintf(req + n, sizeof(req) - n, "\n");
  }
  pending_revision = toggle ? data.rev : draft.rev;
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
static void back(void) {
  if (dirty && !pending) {
    dialog = 2;
  } else {
    page = 0;
    dirty = 0;
    dialog = 0;
  }
  need_draw = 1;
}
static void tap(int x, int y) {
  need_draw = 1;
  if (dialog == 1) {
    if (y >= 300 && y < 356) {
      if (x < 160)
        dialog = 0;
      else
        submit(1, 0);
    }
    return;
  }
  if (dialog == 2) {
    if (y >= 210 && y < 266) {
      return_after_save = 1;
      submit(0, 0);
    } else if (y >= 282 && y < 338) {
      dirty = 0;
      page = 0;
      dialog = 0;
    } else if (y >= 354 && y < 410)
      dialog = 0;
    return;
  }
  if (dialog == 3) {
    if (y >= 300 && y < 356) {
      dialog = 0;
      page = 0;
      dirty = 0;
      notice[0] = 0;
    }
    return;
  }
  if (y < 52 && x >= 210) {
    if (page)
      back();
    else
      quitting = 1;
    return;
  }
  if (!page) {
    if (y >= 282 && y < 338 && !blocked() && data.has) {
      if (data.enabled)
        dialog = 1;
      else
        submit(1, 1);
    } else if (y >= 354 && y < 410 && !pending && !stale()) {
      draft = data;
      for (int i = 0; i < draft.n; i++)
        original_selection[i] = draft.rows[i].selected;
      page = 1;
      offset = 0;
      dirty = 0;
    }
    return;
  }
  if (y >= 416 && y < 468 && x >= 12 && x < 308) {
    if (x < 160)
      back();
    else if (dirty) {
      return_after_save = 0;
      submit(0, 0);
    }
    return;
  }
  if (pending)
    return;
  if (y >= 354 && y < 402) {
    if (x < 108 && offset > 0)
      offset -= 4;
    else if (x >= 212 && offset + 4 < draft.n)
      offset += 4;
    return;
  }
  if (y >= 106 && y < 346 && x >= 12 && x < 308) {
    int i = offset + (y - 106) / 60;
    if (i < draft.n && (y - 106) % 60 < 56) {
      draft.rows[i].selected = !draft.rows[i].selected;
      dirty = 0;
      for (int j = 0; j < draft.n; j++)
        if (draft.rows[j].selected != original_selection[j])
          dirty = 1;
    }
  }
}
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
  int down, sx, sy, moved, multi, dropped;
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
    if (abs(x - t->sx) > 12 || abs(y - t->sy) > 12)
      t->moved = 1;
  } else if (t->down && t->id[0] < 0) {
    if (!t->multi && !t->moved)
      tap(t->sx, t->sy);
    t->down = 0;
    return 1;
  }
  return t->id[0] >= 0;
}
static int input_self_test(void) {
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
  puts("Rotated MT-B tap, drag rejection and dropped-event tests passed");
  return 0;
}
static int panel(int heartbeat, int idle_seconds) {
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
              if (page) {
                draft = data;
                for (int i = 0; i < draft.n; i++)
                  original_selection[i] = draft.rows[i].selected;
                if (return_after_save)
                  page = 0;
              }
              return_after_save = 0;
            }
          }
        }
      }
    }
    if (now - contact > 15000)
      break;
    if (need_draw) {
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
    struct pollfd p = {.fd = input.fd, .events = POLLIN};
    poll(&p, 1, 50);
    struct input_event ev;
    while (read(input.fd, &ev, sizeof(ev)) == sizeof(ev)) {
      if (input_event(&input, &ev))
        last = monotonic_ms();
    }
  }
  result = 0;
end:
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
    puts("openui-screen 1");
    return 0;
  }
  if (argc == 2 && !strcmp(argv[1], "--probe")) {
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
int main(int argc, char **argv) {
  if (argc < 2)
    return 2;
  // Exercise asymmetric corners and row padding independently of UI layout.
  struct drm_buf padded = {.pitch = (W + 3) * 2, .map = calloc((W + 3) * H, 2)};
  assert(padded.map);
  padded.map[0] = 1;
  padded.map[W - 1] = 2;
  padded.map[(W + 3) * (H - 1)] = 3;
  padded.map[(W + 3) * (H - 1) + W - 1] = 4;
  padded.map[W] = 0x1234;
  rotate_frame(&padded);
  assert(padded.map[0] == 4 && padded.map[W - 1] == 3);
  assert(padded.map[(W + 3) * (H - 1)] == 2);
  assert(padded.map[(W + 3) * (H - 1) + W - 1] == 1);
  assert(padded.map[W] == 0x1234);
  rotate_frame(&padded);
  assert(padded.map[0] == 1 && padded.map[W - 1] == 2);
  free(padded.map);
  char example[2048];
  snprintf(
      example, sizeof(example),
      "S 7 1 1 1 0 %llu 134217728 25165824 1 "
      "%llu\nD\t02:11:22:33:44:55\t1\t1\t192.0.2.2\tWork laptop\nD\t02:11:22:"
      "33:44:56\t0\t1\t192.0.2.3\tiPhone\nD\t02:11:22:33:44:57\t1\t0\t-\t-"
      "\nJ 0 0 0 0\n.\n",
      (unsigned long long)time(NULL) - 15, (unsigned long long)time(NULL));
  assert(parse_snapshot(example, &data));
  assert(data.n == 3);
  assert(data.rows[1].selected == 0);
  struct drm_buf b = {.pitch = 640, .map = calloc(W * H, 2)};
  char path[512];
  render(&b);
  snprintf(path, sizeof(path), "%s/home.ppm", argv[1]);
  dump(&b, path);
  rotate_frame(&b);
  snprintf(path, sizeof(path), "%s/home-panel.ppm", argv[1]);
  dump(&b, path);
  tap(30, 370);
  assert(page == 1);
  tap(70, 185);
  assert(dirty);
  assert(draft.rows[1].selected == 1);
  render(&b);
  snprintf(path, sizeof(path), "%s/devices.ppm", argv[1]);
  dump(&b, path);
  tap(230, 20);
  assert(dialog == 2);
  render(&b);
  snprintf(path, sizeof(path), "%s/unsaved.ppm", argv[1]);
  dump(&b, path);
  tap(60, 235);
  assert(strstr(test_request, "devices 7 "));
  assert(pending == 1);
  pending = 0;
  dialog = 0;
  page = 0;
  tap(70, 300);
  assert(dialog == 1);
  render(&b);
  snprintf(path, sizeof(path), "%s/confirm.ppm", argv[1]);
  dump(&b, path);
  tap(220, 320);
  assert(!strcmp(test_request, "toggle 7 0\n"));
  pending = 0;
  dialog = 0;
  data.has = 0;
  data.enabled = 0;
  data.connected = 0;
  render(&b);
  snprintf(path, sizeof(path), "%s/unconfigured.ppm", argv[1]);
  dump(&b, path);
  puts("Screen parser, selection, confirmation and versioned action tests "
       "passed");
  free(b.map);
  return 0;
}
#endif

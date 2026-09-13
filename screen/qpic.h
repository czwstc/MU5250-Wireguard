/* SPDX-License-Identifier: GPL-3.0-or-later
 * Adapted from mu5250_tweaking/files/qpic_demo/qpic_drm_demo.c.
 * Original project LICENSE is preserved in this directory.
 */
/*
 * qpic_drm_demo.c — U60Pro (MU5250) DRM/QPIC screen demo
 *
 * Atomic KMS + dumb RGB565 buffers (matches zte_topsw_devui path).
 * Scenes: RGB → mosaic → clock → touch pointer-location → blank.
 * Build: see build.sh / docs/screen.md
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#if defined(__has_include)
#if __has_include(<libdrm/drm.h>)
#include <libdrm/drm.h>
#include <libdrm/drm_mode.h>
#elif __has_include(<drm/drm.h>)
#include <drm/drm.h>
#include <drm/drm_mode.h>
#else
#include <drm/drm.h>
#include <drm/drm_mode.h>
#endif
#else
#include <drm/drm.h>
#include <drm/drm_mode.h>
#endif

#ifndef DRM_FORMAT_RGB565
#ifndef fourcc_code
#define fourcc_code(a, b, c, d)                                                \
  ((uint32_t)(a) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) |              \
   ((uint32_t)(d) << 24))
#endif
#define DRM_FORMAT_RGB565 fourcc_code('R', 'G', '1', '6')
#endif

#ifndef DRM_MODE_CONNECTED
#define DRM_MODE_CONNECTED 1
#endif

#ifndef DRM_CLIENT_CAP_ATOMIC
#define DRM_CLIENT_CAP_ATOMIC 3
#endif

#ifndef DRM_IOCTL_MODE_DESTROY_BLOB
#ifdef DRM_IOCTL_MODE_DESTROYPROPBLOB
#define DRM_IOCTL_MODE_DESTROY_BLOB DRM_IOCTL_MODE_DESTROYPROPBLOB
#endif
#endif

#ifndef DRM_MODE_OBJECT_PLANE
#define DRM_MODE_OBJECT_PLANE 0x53524150
#endif

#define W 320
#define H 480
#define BPP 16
#define NBUFS 2
#define TOUCH_SLOTS 2
#define TOUCH_TRAIL 512

static volatile sig_atomic_t g_stop;

static void on_sig(int sig) {
  (void)sig;
  g_stop = 1;
}

static void logline(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  vfprintf(stderr, fmt, ap);
  va_end(ap);
  fputc('\n', stderr);
}

static inline uint16_t rgb565(uint8_t r, uint8_t g, uint8_t b) {
  return (uint16_t)(((r & 0xf8) << 8) | ((g & 0xfc) << 3) | (b >> 3));
}

struct drm_buf {
  uint32_t handle;
  uint32_t pitch;
  uint64_t size;
  uint32_t fb_id;
  uint16_t *map;
};

struct drm_props {
  uint32_t plane_fb_id;
  uint32_t plane_crtc_id;
  uint32_t src_x, src_y, src_w, src_h;
  uint32_t crtc_x, crtc_y, crtc_w, crtc_h;
  uint32_t crtc_mode_id;
  uint32_t crtc_active;
  uint32_t conn_crtc_id;
};

struct qpic_ctx {
  int fd;
  uint32_t conn_id;
  uint32_t crtc_id;
  uint32_t plane_id;
  uint32_t mode_blob_id;
  struct drm_mode_modeinfo mode;
  struct drm_props props;
  struct drm_buf bufs[NBUFS];
  int cur_buf;
  int first_commit;
};

static int drm_ioctl(int fd, unsigned long req, void *arg) {
  int ret;
  do {
    ret = ioctl(fd, req, arg);
  } while (ret == -1 && (errno == EINTR || errno == EAGAIN));
  return ret;
}

static int drm_set_client_cap(int fd, uint64_t cap, uint64_t val) {
  struct drm_set_client_cap arg = {.capability = cap, .value = val};
  return drm_ioctl(fd, DRM_IOCTL_SET_CLIENT_CAP, &arg);
}

static int drm_get_cap(int fd, uint64_t cap, uint64_t *val) {
  struct drm_get_cap arg = {.capability = cap, .value = 0};
  if (drm_ioctl(fd, DRM_IOCTL_GET_CAP, &arg) < 0)
    return -1;
  *val = arg.value;
  return 0;
}

static int drm_find_prop_id(int fd, uint32_t obj_id, uint32_t obj_type,
                            const char *want) {
  struct drm_mode_obj_get_properties gprops;
  struct drm_mode_get_property gprop;
  uint32_t *ids = NULL;
  uint64_t *vals = NULL;
  uint32_t count = 0;
  int i, ret = -1;

  for (;;) {
    memset(&gprops, 0, sizeof(gprops));
    gprops.obj_id = obj_id;
    gprops.obj_type = obj_type;
    gprops.count_props = count;
    if (count) {
      gprops.props_ptr = (uint64_t)(uintptr_t)ids;
      gprops.prop_values_ptr = (uint64_t)(uintptr_t)vals;
    }
    if (drm_ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gprops) < 0)
      goto out;
    if (gprops.count_props <= count)
      break;
    count = gprops.count_props;
    free(ids);
    free(vals);
    ids = calloc(count, sizeof(uint32_t));
    vals = calloc(count, sizeof(uint64_t));
    if (!ids || !vals)
      goto out;
  }

  for (i = 0; i < (int)count; i++) {
    memset(&gprop, 0, sizeof(gprop));
    gprop.prop_id = ids[i];
    if (drm_ioctl(fd, DRM_IOCTL_MODE_GETPROPERTY, &gprop) < 0)
      continue;
    if (strncmp(gprop.name, want, sizeof(gprop.name)) == 0) {
      ret = (int)ids[i];
      break;
    }
  }

out:
  free(ids);
  free(vals);
  return ret;
}

static int drm_map_props(int fd, struct qpic_ctx *d) {
  struct drm_props *p = &d->props;
  const struct {
    const char *name;
    uint32_t obj;
    uint32_t type;
    uint32_t *dst;
  } tbl[] = {
      {"FB_ID", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->plane_fb_id},
      {"CRTC_ID", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->plane_crtc_id},
      {"SRC_X", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->src_x},
      {"SRC_Y", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->src_y},
      {"SRC_W", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->src_w},
      {"SRC_H", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->src_h},
      {"CRTC_X", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->crtc_x},
      {"CRTC_Y", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->crtc_y},
      {"CRTC_W", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->crtc_w},
      {"CRTC_H", d->plane_id, DRM_MODE_OBJECT_PLANE, &p->crtc_h},
      {"MODE_ID", d->crtc_id, DRM_MODE_OBJECT_CRTC, &p->crtc_mode_id},
      {"ACTIVE", d->crtc_id, DRM_MODE_OBJECT_CRTC, &p->crtc_active},
      {"CRTC_ID", d->conn_id, DRM_MODE_OBJECT_CONNECTOR, &p->conn_crtc_id},
  };
  size_t i;

  for (i = 0; i < sizeof(tbl) / sizeof(tbl[0]); i++) {
    int pid = drm_find_prop_id(fd, tbl[i].obj, tbl[i].type, tbl[i].name);
    if (pid < 0) {
      logline("property %s not found on obj %u", tbl[i].name, tbl[i].obj);
      return -1;
    }
    *tbl[i].dst = (uint32_t)pid;
    logline("prop %s id=%u", tbl[i].name, (uint32_t)pid);
  }
  return 0;
}

static int crtc_index_of(uint32_t *crtc_ids, uint32_t count, uint32_t crtc_id) {
  uint32_t i;

  for (i = 0; i < count; i++) {
    if (crtc_ids[i] == crtc_id)
      return (int)i;
  }
  return -1;
}

static int drm_find_objects(struct qpic_ctx *d) {
  struct drm_mode_card_res res;
  struct drm_mode_get_plane_res pres;
  struct drm_mode_modeinfo tmp_mode;
  uint32_t *conn_ids = NULL, *crtc_ids = NULL, *plane_ids = NULL;
  int i, j, ok = -1;
  int crtc_idx = -1;

  memset(&res, 0, sizeof(res));
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0) {
    logline("GETRESOURCES probe: %s", strerror(errno));
    return -1;
  }
  logline("GETRESOURCES: %u connectors, %u crtcs, %u encoders",
          res.count_connectors, res.count_crtcs, res.count_encoders);
  if (!res.count_connectors || !res.count_crtcs)
    return -1;

  conn_ids = calloc(res.count_connectors, sizeof(uint32_t));
  crtc_ids = calloc(res.count_crtcs, sizeof(uint32_t));
  plane_ids = NULL;
  if (!conn_ids || !crtc_ids)
    goto out;

  res.connector_id_ptr = (uint64_t)(uintptr_t)conn_ids;
  res.crtc_id_ptr = (uint64_t)(uintptr_t)crtc_ids;
  if (res.count_encoders) {
    uint32_t *enc_ids = calloc(res.count_encoders, sizeof(uint32_t));
    if (!enc_ids)
      goto out;
    res.encoder_id_ptr = (uint64_t)(uintptr_t)enc_ids;
  }
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0) {
    logline("GETRESOURCES fill: %s", strerror(errno));
    goto out;
  }
  if (res.encoder_id_ptr)
    free((void *)(uintptr_t)res.encoder_id_ptr);

  for (i = 0; i < (int)res.count_crtcs; i++)
    logline("crtc[%d] id=%u", i, crtc_ids[i]);

  for (i = 0; i < (int)res.count_connectors; i++) {
    struct drm_mode_get_connector c;
    struct drm_mode_modeinfo *modes = NULL;
    uint32_t *encs = NULL;
    uint32_t prev_modes = 0;

    memset(&c, 0, sizeof(c));
    c.connector_id = conn_ids[i];
    c.count_modes = 1;
    c.modes_ptr = (uint64_t)(uintptr_t)&tmp_mode;
    if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETCONNECTOR, &c) < 0) {
      logline("conn %u GETCONNECTOR probe: %s", conn_ids[i], strerror(errno));
      continue;
    }
    logline(
        "conn %u type=%u conn=%u modes=%u encoders=%u props=%u encoder_id=%u",
        c.connector_id, c.connector_type, c.connection, c.count_modes,
        c.count_encoders, c.count_props, c.encoder_id);
    if (c.connection != DRM_MODE_CONNECTED || !c.count_modes)
      continue;

    do {
      uint32_t *props = NULL;
      uint64_t *prop_vals = NULL;

      prev_modes = c.count_modes;
      free(modes);
      free(encs);
      modes = calloc(c.count_modes, sizeof(*modes));
      encs = calloc(c.count_encoders ? c.count_encoders : 1, sizeof(uint32_t));
      if (!modes || !encs)
        goto next_conn;
      if (c.count_props) {
        props = calloc(c.count_props, sizeof(uint32_t));
        prop_vals = calloc(c.count_props, sizeof(uint64_t));
        if (!props || !prop_vals) {
          free(props);
          free(prop_vals);
          goto next_conn;
        }
      }

      c.modes_ptr = (uint64_t)(uintptr_t)modes;
      c.encoders_ptr = (uint64_t)(uintptr_t)encs;
      c.props_ptr = props ? (uint64_t)(uintptr_t)props : 0;
      c.prop_values_ptr = prop_vals ? (uint64_t)(uintptr_t)prop_vals : 0;
      if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETCONNECTOR, &c) < 0) {
        logline("conn %u GETCONNECTOR fill: %s", conn_ids[i], strerror(errno));
        free(props);
        free(prop_vals);
        goto next_conn;
      }
      free(props);
      free(prop_vals);
    } while (c.count_modes != prev_modes);

    for (j = 0; j < (int)c.count_modes; j++) {
      if (modes[j].hdisplay == W && modes[j].vdisplay == H)
        break;
    }
    if (j == (int)c.count_modes)
      j = 0;

    memcpy(&d->mode, &modes[j], sizeof(d->mode));
    d->conn_id = c.connector_id;
    logline("selected mode %ux%u@%u on conn %u", d->mode.hdisplay,
            d->mode.vdisplay, d->mode.vrefresh, d->conn_id);

    {
      struct drm_mode_get_encoder enc;
      uint32_t enc_id =
          c.encoder_id ? c.encoder_id : (c.count_encoders ? encs[0] : 0);

      d->crtc_id = 0;
      if (enc_id) {
        memset(&enc, 0, sizeof(enc));
        enc.encoder_id = enc_id;
        if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETENCODER, &enc) == 0) {
          if (enc.crtc_id)
            d->crtc_id = enc.crtc_id;
          else {
            for (j = 0; j < (int)res.count_crtcs; j++) {
              if (enc.possible_crtcs & (1u << j)) {
                d->crtc_id = crtc_ids[j];
                break;
              }
            }
          }
        }
      }
      if (!d->crtc_id && res.count_crtcs)
        d->crtc_id = crtc_ids[0];
    }

    crtc_idx = crtc_index_of(crtc_ids, res.count_crtcs, d->crtc_id);
    logline("crtc_id=%u (index %d)", d->crtc_id, crtc_idx);
    if (crtc_idx < 0)
      goto next_conn;

    memset(&pres, 0, sizeof(pres));
    if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pres) < 0 ||
        !pres.count_planes) {
      logline("GETPLANERESOURCES: %s count=%u", strerror(errno),
              pres.count_planes);
      goto next_conn;
    }

    free(plane_ids);
    plane_ids = calloc(pres.count_planes, sizeof(uint32_t));
    if (!plane_ids)
      goto next_conn;
    pres.plane_id_ptr = (uint64_t)(uintptr_t)plane_ids;
    if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pres) < 0) {
      logline("GETPLANERESOURCES fill: %s", strerror(errno));
      goto next_conn;
    }
    logline("%u planes", pres.count_planes);

    for (j = 0; j < (int)pres.count_planes; j++) {
      struct drm_mode_get_plane pl;
      uint32_t *formats = NULL;
      int k;

      memset(&pl, 0, sizeof(pl));
      pl.plane_id = plane_ids[j];
      if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETPLANE, &pl) < 0) {
        logline("plane %u GETPLANE probe: %s", plane_ids[j], strerror(errno));
        continue;
      }
      logline("plane %u possible_crtcs=0x%x count_formats=%u", plane_ids[j],
              pl.possible_crtcs, pl.count_format_types);
      if (!(pl.possible_crtcs & (1u << crtc_idx)))
        continue;

      formats = calloc(pl.count_format_types ? pl.count_format_types : 1,
                       sizeof(uint32_t));
      if (!formats)
        continue;
      pl.format_type_ptr = (uint64_t)(uintptr_t)formats;
      if (drm_ioctl(d->fd, DRM_IOCTL_MODE_GETPLANE, &pl) < 0) {
        logline("plane %u GETPLANE fill: %s", plane_ids[j], strerror(errno));
        free(formats);
        continue;
      }
      for (k = 0; k < (int)pl.count_format_types; k++) {
        logline("  plane %u fmt=0x%08x", plane_ids[j], formats[k]);
        if (formats[k] == DRM_FORMAT_RGB565) {
          d->plane_id = plane_ids[j];
          logline("plane %u supports RGB565", d->plane_id);
          free(formats);
          ok = 0;
          goto out;
        }
      }
      free(formats);
    }
    continue;

  next_conn:
    free(modes);
    free(encs);
    modes = NULL;
    encs = NULL;
    d->crtc_id = 0;
  }

out:
  free(conn_ids);
  free(crtc_ids);
  free(plane_ids);
  if (ok == 0)
    logline("conn=%u crtc=%u plane=%u mode=%ux%u", d->conn_id, d->crtc_id,
            d->plane_id, d->mode.hdisplay, d->mode.vdisplay);
  return ok;
}

static int drm_create_mode_blob(struct qpic_ctx *d) {
  struct drm_mode_create_blob blob;

  memset(&blob, 0, sizeof(blob));
  blob.length = sizeof(d->mode);
  blob.data = (uint64_t)(uintptr_t)&d->mode;
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_CREATEPROPBLOB, &blob) < 0) {
    logline("CREATEPROPBLOB: %s", strerror(errno));
    return -1;
  }
  d->mode_blob_id = blob.blob_id;
  logline("mode blob id=%u", d->mode_blob_id);
  return 0;
}

static int drm_buf_create(struct qpic_ctx *d, struct drm_buf *b) {
  struct drm_mode_create_dumb creq;
  struct drm_mode_map_dumb mreq;
  struct drm_mode_fb_cmd2 fb;

  memset(&creq, 0, sizeof(creq));
  creq.width = W;
  creq.height = H;
  creq.bpp = BPP;
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_CREATE_DUMB, &creq) < 0) {
    logline("CREATE_DUMB: %s", strerror(errno));
    return -1;
  }
  b->handle = creq.handle;
  b->pitch = creq.pitch;
  b->size = creq.size;

  memset(&fb, 0, sizeof(fb));
  fb.width = W;
  fb.height = H;
  fb.pixel_format = DRM_FORMAT_RGB565;
  fb.handles[0] = b->handle;
  fb.pitches[0] = b->pitch;
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_ADDFB2, &fb) < 0) {
    logline("ADDFB2: %s", strerror(errno));
    return -1;
  }
  b->fb_id = fb.fb_id;

  memset(&mreq, 0, sizeof(mreq));
  mreq.handle = b->handle;
  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_MAP_DUMB, &mreq) < 0) {
    logline("MAP_DUMB: %s", strerror(errno));
    return -1;
  }
  b->map = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, d->fd,
                mreq.offset);
  if (b->map == MAP_FAILED) {
    logline("mmap: %s", strerror(errno));
    b->map = NULL;
    return -1;
  }
  memset(b->map, 0, b->size);
  logline("buf fb_id=%u pitch=%u map=%p", b->fb_id, b->pitch, b->map);
  return 0;
}

static void drm_buf_destroy(struct qpic_ctx *d, struct drm_buf *b) {
  if (b->map && b->map != MAP_FAILED) {
    munmap(b->map, b->size);
    b->map = NULL;
  }
  if (b->fb_id) {
    drm_ioctl(d->fd, DRM_IOCTL_MODE_RMFB, &b->fb_id);
    b->fb_id = 0;
  }
  if (b->handle) {
    struct drm_mode_destroy_dumb req = {.handle = b->handle};
    drm_ioctl(d->fd, DRM_IOCTL_MODE_DESTROY_DUMB, &req);
    b->handle = 0;
  }
  (void)d;
}

static int drm_atomic_commit(struct qpic_ctx *d, struct drm_buf *b, int x,
                             int y, int w, int h, uint32_t flags) {
  struct drm_mode_atomic atomic;
  struct drm_props *p = &d->props;
  uint32_t objs[32];
  uint32_t counts[32];
  uint32_t props[32];
  uint64_t values[32];
  int n = 0;
  uint32_t src_w = (uint32_t)w << 16;
  uint32_t src_h = (uint32_t)h << 16;

#define ADD_PROP(obj, prop, val)                                               \
  do {                                                                         \
    objs[n] = (obj);                                                           \
    counts[n] = 1;                                                             \
    props[n] = (prop);                                                         \
    values[n] = (val);                                                         \
    n++;                                                                       \
  } while (0)

  ADD_PROP(d->plane_id, p->plane_fb_id, b->fb_id);
  ADD_PROP(d->plane_id, p->plane_crtc_id, d->crtc_id);
  ADD_PROP(d->plane_id, p->src_x, (uint32_t)x << 16);
  ADD_PROP(d->plane_id, p->src_y, (uint32_t)y << 16);
  ADD_PROP(d->plane_id, p->src_w, src_w);
  ADD_PROP(d->plane_id, p->src_h, src_h);
  ADD_PROP(d->plane_id, p->crtc_x, (uint32_t)x);
  ADD_PROP(d->plane_id, p->crtc_y, (uint32_t)y);
  ADD_PROP(d->plane_id, p->crtc_w, (uint32_t)w);
  ADD_PROP(d->plane_id, p->crtc_h, (uint32_t)h);

  if (d->first_commit) {
    ADD_PROP(d->crtc_id, p->crtc_active, 1);
    ADD_PROP(d->crtc_id, p->crtc_mode_id, d->mode_blob_id);
    ADD_PROP(d->conn_id, p->conn_crtc_id, d->crtc_id);
    flags |= DRM_MODE_ATOMIC_ALLOW_MODESET;
  }

#undef ADD_PROP

  memset(&atomic, 0, sizeof(atomic));
  atomic.flags = flags;
  atomic.count_objs = (uint32_t)n;
  atomic.objs_ptr = (uint64_t)(uintptr_t)objs;
  atomic.count_props_ptr = (uint64_t)(uintptr_t)counts;
  atomic.props_ptr = (uint64_t)(uintptr_t)props;
  atomic.prop_values_ptr = (uint64_t)(uintptr_t)values;

  if (drm_ioctl(d->fd, DRM_IOCTL_MODE_ATOMIC, &atomic) < 0) {
    logline("ATOMIC: %s", strerror(errno));
    return -1;
  }
  d->first_commit = 0;

  return 0;
}

static int drm_present(struct qpic_ctx *d) {
  struct drm_buf *b = &d->bufs[d->cur_buf];
  return drm_atomic_commit(d, b, 0, 0, W, H, 0);
}

static void drm_destroy(struct qpic_ctx *d) {
  int i;

  if (d->mode_blob_id) {
#ifdef DRM_IOCTL_MODE_DESTROY_BLOB
    struct drm_mode_destroy_blob req = {.blob_id = d->mode_blob_id};
    drm_ioctl(d->fd, DRM_IOCTL_MODE_DESTROY_BLOB, &req);
#else
    (void)d;
#endif
    d->mode_blob_id = 0;
  }
  for (i = 0; i < NBUFS; i++)
    drm_buf_destroy(d, &d->bufs[i]);
  if (d->fd >= 0) {
    drm_ioctl(d->fd, DRM_IOCTL_DROP_MASTER, 0);
    close(d->fd);
    d->fd = -1;
  }
}

static int drm_init(struct qpic_ctx *d) {
  uint64_t cap;
  const char *dev;
  int i;

  memset(d, 0, sizeof(*d));
  d->fd = -1;
  d->first_commit = 1;

  dev = getenv("DRM_CARD");
  if (!dev || !dev[0])
    dev = "/dev/dri/card0";

  d->fd = open(dev, O_RDWR | O_CLOEXEC);
  if (d->fd < 0) {
    logline("open %s: %s", dev, strerror(errno));
    return -1;
  }
  logline("opened %s", dev);

  if (drm_get_cap(d->fd, DRM_CAP_DUMB_BUFFER, &cap) < 0 || !cap) {
    logline("no dumb buffer support");
    return -1;
  }
  if (drm_set_client_cap(d->fd, DRM_CLIENT_CAP_ATOMIC, 1) < 0) {
    logline("SET_CLIENT_CAP atomic: %s", strerror(errno));
    return -1;
  }
  logline("atomic client cap ok");

  if (drm_ioctl(d->fd, DRM_IOCTL_SET_MASTER, 0) < 0)
    logline("SET_MASTER: %s (continuing)", strerror(errno));
  else
    logline("drm master acquired");

  if (drm_find_objects(d) < 0) {
    logline("find connector/plane failed");
    return -1;
  }
  if (drm_map_props(d->fd, d) < 0)
    return -1;
  if (drm_create_mode_blob(d) < 0)
    return -1;

  for (i = 0; i < NBUFS; i++) {
    if (drm_buf_create(d, &d->bufs[i]) < 0)
      return -1;
  }
  d->cur_buf = 0;
  logline("drm init ok, %d buffers", NBUFS);
  return 0;
}

static struct drm_buf *drm_draw_buf(struct qpic_ctx *d) {
  return &d->bufs[d->cur_buf];
}

static int drm_flip(struct qpic_ctx *d) {
  if (drm_present(d) < 0)
    return -1;
  d->cur_buf ^= 1;
  return 0;
}

static inline uint16_t *px(struct drm_buf *b, int x, int y) {
  return (uint16_t *)((uint8_t *)b->map + y * b->pitch) + x;
}

static void fill(struct drm_buf *b, uint16_t c) {
  int y, x;
  for (y = 0; y < H; y++) {
    uint16_t *row = (uint16_t *)((uint8_t *)b->map + y * b->pitch);
    for (x = 0; x < W; x++)
      row[x] = c;
  }
}

static void fill_rect(struct drm_buf *b, int x0, int y0, int x1, int y1,
                      uint16_t c) {
  int x, y;
  if (x0 < 0)
    x0 = 0;
  if (y0 < 0)
    y0 = 0;
  if (x1 > W)
    x1 = W;
  if (y1 > H)
    y1 = H;
  for (y = y0; y < y1; y++) {
    uint16_t *row = (uint16_t *)((uint8_t *)b->map + y * b->pitch);
    for (x = x0; x < x1; x++)
      row[x] = c;
  }
}

static void draw_pixel(struct drm_buf *b, int x, int y, uint16_t c) {
  if (x >= 0 && x < W && y >= 0 && y < H)
    *px(b, x, y) = c;
}

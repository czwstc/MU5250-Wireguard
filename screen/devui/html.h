/* DevUI/litehtml bridge; Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
extern void html_view_init(uint16_t *, int, int, int, int, const char *);
extern int html_view_render_to(uint16_t *, const char *);
extern const char *html_view_click(float, float);
extern int html_view_rect(const char *, int *, int *, int *, int *);
extern int html_view_text_width_px(const char *, int);

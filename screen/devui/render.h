/* DevUI HTML/CSS pages rendered by the upstream litehtml host.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static char html[49152];
static size_t html_used;
static void add(const char *fmt, ...) {
  if (html_used >= sizeof(html) - 1) return;
  va_list args;
  va_start(args, fmt);
  int n = vsnprintf(html + html_used, sizeof(html) - html_used, fmt, args);
  va_end(args);
  if (n > 0) html_used += (size_t)n < sizeof(html) - html_used ? (size_t)n : sizeof(html) - html_used - 1;
}
static void escaped(const char *s) {
  for (; *s; s++) {
    switch (*s) {
      case '&': add("&amp;"); break;
      case '<': add("&lt;"); break;
      case '>': add("&gt;"); break;
      case '"': add("&quot;"); break;
      case '\'': add("&#39;"); break;
      case '\n': add("<br>"); break; /* litehtml does not honour pre-line */
      default: add("%c", *s);
    }
  }
}
static void shortened(const char *text, int width) {
  char label[512];snprintf(label,sizeof(label),"%s",text);
  if(html_view_text_width_px(label,18)>width) {
    size_t n=strlen(label);
    while(n>0) {
      do { n--; } while(n>0&&((unsigned char)label[n]&0xc0)==0x80);
      strcpy(label+n,"...");
      if(html_view_text_width_px(label,18)<=width)break;
    }
  }
  escaped(label);
}
static void box(int x, int y, int w, int h, const char *cls, const char *label) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">", cls, x,y,w,h);
  escaped(label); add("</div>");
}
static void linkbox(int x, int y, int w, int h, const char *id, const char *label, int enabled, int primary) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box btn %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;line-height:%dpx\">",id,enabled?id:"none",!enabled?"disabled":primary?"primary":"",x,y,w,h,h-2);
  shortened(label,w-24); add("</a>");
}
/* Two-line row: name on top, detail below; the caller closes it with row_end(). */
static void row_begin(int x, int y, int w, int h, const char *id, int enabled, int current, const char *name, int name_width) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box row%s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\"><div class=\"name\">",
      id, enabled ? id : "none", current ? " current" : "", x, y, w, h);
  shortened(name, name_width); add("</div>");
}
static void row_end(void) { add("</a>"); }
static const char *tone_class(int tone) {
  return tone == 1 ? "good" : tone == 2 ? "warn" : tone == 3 ? "bad" : "grey";
}
static const char *summary(void) {
  return !data.stamp ? "Loading status" : stale() ? "Status is stale" :
    pending || data.busy ? "Applying changes" : !data.has ? "No configuration" :
    data.error ? "Tunnel error" : !data.enabled ? "Off" :
    data.connected ? "Recent handshake" : "Waiting for handshake";
}
static int wg_tone(void) {
  return data.error || stale() ? 3 : !data.enabled || !data.has ? 0 : data.connected ? 1 : 2;
}
static const char *sb_summary(int *tone) {
  *tone = 0;
  if (!data.sb_ok) return "Loading status";
  if (!data.sb_installed) return "Not installed";
  if (!data.sb_enabled) return "Off";
  if (data.sb_error && !data.sb_starting) { *tone = 3; return "Needs attention"; }
  if (!data.sb_ready) { *tone = 2; return "Starting..."; }
  *tone = 1; return "Running";
}
static const char *active_profile(void) {
  for (int i=0;i<data.np;i++) if(data.profiles[i].active) return data.profiles[i].name;
  return "None selected";
}
static const char *sb_active_profile(void) {
  for (int i=0;i<data.nq;i++) if(data.sb_profiles[i].active) return data.sb_profiles[i].name;
  return "None selected";
}
static int selected_count(int sb) {
  int n=0;for(int i=0;i<data.n;i++) n+=sb?data.rows[i].sb:data.rows[i].selected;return n;
}
static void header(const char *title) {
  box(16,18,188,32,"title",title);
  linkbox(210,6,100,50,"back","Back",1,0);
}
static void battery_icon(int x, int y) {
  int known = data.battery >= 0, percent = !known ? 0 : data.battery > 100 ? 100 : data.battery;
  const char *color = !known ? "#a7b6ca" : percent <= 20 ? "#ff9999" : "#64e2aa";
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:58px;height:28px;border:2px solid %s;border-radius:4px\"></div>",x,y,color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:4px;height:10px;background:%s\"></div>",x+58,y+9,color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:20px;background:%s\"></div>",x+4,y+4,50*percent/100,color);
  char label[16];
  if(!known)snprintf(label,sizeof(label),"--%%");else snprintf(label,sizeof(label),"%d%%",percent);
  box(x+70,y-8,84,44,!known?"pct grey":percent<=20?"pct bad":"pct",label);
}
static void signal_bars(int x, int y) {
  int n = data.sig_ok ? data.sig_bars : 0;
  const char *on = n >= 4 ? "#64e2aa" : n >= 2 ? "#ffc77b" : "#ff9999";
  for (int i = 0; i < 5; i++) {
    int h = 8 + i * 5;
    add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:6px;height:%dpx;background:%s\"></div>", x + i * 9, y + 28 - h, h, i < n ? on : "#3a475a");
  }
}
static void gateway_row(int y, const char *id, const char *name, const char *status, int tone, const char *detail) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box row\" style=\"left:12px;top:%dpx;width:296px;height:78px\">", id, id, y);
  add("<div class=\"name\">"); escaped(name); add("</div>");
  add("<div class=\"box %s\" style=\"left:120px;top:8px;width:146px;height:22px;text-align:right;font-size:15px\">", tone_class(tone));
  char label[64]; snprintf(label, sizeof(label), "\xe2\x97\x8f %s", status); escaped(label); add("</div>");
  add("<div class=\"box sub\" style=\"left:0;top:46px;width:250px;height:22px\">"); shortened(detail, 238); add("</div>");
  add("<div class=\"box chev\" style=\"left:272px;top:24px;width:16px;height:28px\">&gt;</div></a>");
}
static void overview_page(void) {
  char tmp[160], up[32];
  box(16,18,188,32,"title","MU5250");
  linkbox(210,6,100,50,"stock","Stock UI",1,0);
  box(12,64,296,132,"card","");
  box(24,72,120,18,"small","BATTERY");
  battery_icon(24,100);
  if (data.battery < 0) strcpy(tmp, "Battery --");
  else if (data.temperature != -999) snprintf(tmp,sizeof(tmp),"%.1f\xc2\xb0""C \xc2\xb7 %s",data.temperature/10.0,data.charging?"Charging":"On battery");
  else snprintf(tmp,sizeof(tmp),"%s",data.charging?"Charging":"On battery");
  box(24,140,138,18,"small",tmp);
  box(166,76,1,82,"divider","");
  box(178,72,120,18,"small","SIGNAL");
  signal_bars(178,98);
  if (data.sig_ok && strcmp(data.sig_type, "-")) {
    box(226,96,82,26,"sigtype",data.sig_type);
    if (data.sig_rsrp) snprintf(tmp,sizeof(tmp),"%d dBm",data.sig_rsrp); else strcpy(tmp,"-- dBm");
    box(226,121,82,18,"small",tmp);
    add("<div class=\"box small\" style=\"left:178px;top:140px;width:124px;height:18px\">");
    shortened(data.sig_operator,120); add("</div>");
  } else {
    box(226,101,82,22,"nosig grey","No signal");
    box(178,140,124,18,"small",data.sig_ok?"No service":"Signal unknown");
  }
  snprintf(up,sizeof(up),"%lluh %llum",data.uptime/3600,data.uptime/60%60);
  if (data.memory >= 0) snprintf(tmp,sizeof(tmp),"CPU %.0f%% \xc2\xb7 Mem %.0f%% \xc2\xb7 Up %s",data.cpu,data.memory,up);
  else snprintf(tmp,sizeof(tmp),"CPU %.0f%% \xc2\xb7 Up %s",data.cpu,up);
  box(24,168,272,20,"small",tmp);
  int y = 230;
  if (visible_page(SB) || visible_page(WG)) box(16,206,200,18,"small","GATEWAYS");
  if (visible_page(SB)) {
    int tone; const char *state = sb_summary(&tone);
    if (!data.sb_ok || !data.sb_installed) strcpy(tmp, "Install sing-box in OpenUI");
    else if (data.sb_enabled && data.sb_ready && data.node_now >= 0 && data.node_now < data.nn)
      snprintf(tmp,sizeof(tmp),"%s \xc2\xb7 %s",data.nodes[data.node_now].name,sb_active_profile());
    else if (data.sb_enabled && !data.sb_ready && !data.sb_error) strcpy(tmp, "Downloading rule sets...");
    else snprintf(tmp,sizeof(tmp),"Profile: %s",sb_active_profile());
    gateway_row(y, "singbox", "sing-box", state, tone, tmp);
    y += 88;
  }
  if (visible_page(WG)) {
    const char *state = !data.stamp ? "Loading" : !data.has ? "No configuration" : !data.enabled ? "Off" : data.connected ? "Recent handshake" : "Waiting";
    if (!data.has) strcpy(tmp, "Import a config in OpenUI");
    else if (data.enabled && data.connected && data.handshake)
      snprintf(tmp,sizeof(tmp),"%s \xc2\xb7 %llu s ago",active_profile(),(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
    else snprintf(tmp,sizeof(tmp),"Profile: %s",active_profile());
    gateway_row(y, "wireguard", "WireGuard", state, wg_tone(), tmp);
  }
  snprintf(tmp,sizeof(tmp),"Short power press: sleep / wake.\nStock UI after %d seconds idle.",settings_idle);
  box(16,418,294,50,"footer",tmp);
}
static void singbox_page(void) {
  char tmp[192], rx[32], tx[32];
  int tone; const char *state = sb_summary(&tone);
  header("sing-box");
  box(12,66,296,118,"card","");
  snprintf(tmp,sizeof(tmp),"title %s",tone_class(tone));
  box(24,76,272,30,tmp,state);
  snprintf(tmp,sizeof(tmp),"Profile: %s",sb_active_profile());
  add("<div class=\"box small\" style=\"left:24px;top:110px;width:272px;height:20px\">"); shortened(tmp,260); add("</div>");
  if (data.enabled && !data.sb_enabled) box(24,134,272,40,"small warn","WireGuard is on.\nTurn it off first.");
  else if (data.sb_enabled && data.sb_ready) {
    unsigned long long up = data.sb_uptime;
    snprintf(tmp,sizeof(tmp),"Up %lluh %llum \xc2\xb7 Memory %llu MB",up/3600,up/60%60,data.sb_memory/1048576);
    box(24,132,272,20,"small",tmp);
    amount(rx,sizeof(rx),data.sb_rx);amount(tx,sizeof(tx),data.sb_tx);
    snprintf(tmp,sizeof(tmp),"\xe2\x86\x93 %s   \xe2\x86\x91 %s",rx,tx);
    box(24,154,272,20,"small",tmp);
  } else if (data.sb_enabled && data.sb_error) box(24,132,272,40,"small bad","Not running. Restarting\nautomatically; see OpenUI logs.");
  else if (data.sb_enabled) box(24,132,272,40,"small","Downloading rule sets.\nSelected devices wait until ready.");
  else box(24,132,272,40,"small",data.sb_installed?"Devices use normal internet.":"Deploy sing-box from OpenUI.");
  int has_config = data.nq > 0 && strcmp(sb_active_profile(), "None selected");
  int can_toggle = !blocked() && data.sb_ok && data.sb_installed && (data.sb_enabled || (has_config && !data.enabled));
  linkbox(12,194,296,52,"sbtoggle",data.sb_enabled?"Disable sing-box":"Enable sing-box",can_toggle,1);
  if (data.sb_enabled && data.sb_ready && data.node_now >= 0 && data.node_now < data.nn) {
    const struct node *n = &data.nodes[data.node_now];
    if (n->delay > 0) snprintf(tmp,sizeof(tmp),"Node: %s \xc2\xb7 %d ms  >",n->name,n->delay);
    else snprintf(tmp,sizeof(tmp),"Node: %s  >",n->name);
  } else strcpy(tmp, data.sb_enabled && data.sb_ready ? "Nodes  >" : "Node: when running");
  linkbox(12,256,296,50,"nodes",tmp,data.sb_enabled&&data.sb_ready&&data.nn>0,0);
  snprintf(tmp,sizeof(tmp),"Profile: %s  >",sb_active_profile());
  linkbox(12,314,296,50,"sbprofiles",tmp,1,0);
  snprintf(tmp,sizeof(tmp),"Device routing  %d / %d  >",selected_count(1),data.n);
  linkbox(12,372,296,50,"sbdevices",tmp,1,0);
  box(16,434,294,18,"footer",data.sb_all?"New devices: sing-box":"New devices: normal network");
  if(notice[0])box(16,454,294,20,"footer bad",notice);
}
static void delay_label(char *out, size_t n, int delay) {
  if (delay < 0) out[0] = 0;
  else if (delay == 0) snprintf(out, n, " \xc2\xb7 <span class=\"bad\">timeout</span>");
  else snprintf(out, n, " \xc2\xb7 <span class=\"%s\">%d ms</span>", delay < 300 ? "good" : delay < 800 ? "warn" : "bad", delay);
}
static void nodes_page(void) {
  char tmp[192], delay[96];
  header("Nodes");
  snprintf(tmp,sizeof(tmp),"Group: %s \xc2\xb7 tap to switch",data.group);
  add("<div class=\"box small\" style=\"left:16px;top:66px;width:290px;height:22px\">"); shortened(tmp,286); add("</div>");
  for (int i = 0; i < 4 && node_offset + i < data.nn; i++) {
    const struct node *n = &data.nodes[node_offset + i];
    int current = node_offset + i == data.node_now;
    char id[8]; snprintf(id, sizeof(id), "n%d", i);
    snprintf(tmp, sizeof(tmp), "%s%s", current ? "\xe2\x9c\x93 " : "", n->name);
    row_begin(12, 94 + i * 64, 296, 56, id, !blocked() && !current, current, tmp, current ? 186 : 270);
    if (current) add("<div class=\"box small\" style=\"left:218px;top:7px;width:66px;height:18px;text-align:right\">Current</div>");
    add("<div class=\"sub\">");
    snprintf(tmp, sizeof(tmp), "%s%s%s", n->type, strcmp(n->pick, "-") ? " \xe2\x86\x92 " : "", strcmp(n->pick, "-") ? n->pick : "");
    escaped(tmp);
    delay_label(delay, sizeof(delay), n->delay); add("%s", delay);
    add("</div>"); row_end();
  }
  if (!data.nn) box(24,160,272,48,"small","No node group. Start sing-box first.");
  linkbox(12,356,96,48,"nprev","Previous",node_offset>0,0);
  snprintf(tmp,sizeof(tmp),"%d / %d",node_offset/4+1,data.nn?(data.nn+3)/4:1);
  box(120,370,80,22,"small center",tmp);
  linkbox(212,356,96,48,"nnext","Next",node_offset+4<data.nn,0);
  linkbox(12,416,296,52,"test",pending?"Testing...":"Test latency",!blocked()&&data.nn>0,0);
}
static void profiles_page(int sb) {
  char tmp[256];
  int count = sb ? data.nq : data.np;
  header("Saved profiles");
  snprintf(tmp,sizeof(tmp),"%d / 5 saved \xc2\xb7 %s",count,sb?"sing-box":"WireGuard");
  box(16,66,290,22,"small",tmp);
  for (int i = 0; i < count; i++) {
    const struct profile *p = sb ? &data.sb_profiles[i] : &data.profiles[i];
    char id[8]; snprintf(id,sizeof(id),"p%d",i);
    snprintf(tmp,sizeof(tmp),"%s%s",p->active?"[Active] ":"",p->name);
    row_begin(12,96+i*64,296,56,id,!blocked()&&!p->active,p->active,tmp,270);
    if (p->active) strcpy(tmp, "Current profile");
    else if (sb) snprintf(tmp,sizeof(tmp),"%s \xc2\xb7 %d nodes",p->source=='s'?"Subscription":"File",p->nodes);
    else strcpy(tmp, "Tap to switch");
    add("<div class=\"sub\">"); escaped(tmp); add("</div>"); row_end();
  }
  if (!count) box(24,144,272,100,"",sb?"Import sing-box profiles in the OpenUI webpage first.":"Import profiles in the OpenUI webpage first.");
  box(16,420,290,48,"footer",stale()?"Status is stale. Controls are disabled.":sb?"Switching restarts sing-box.\nRouting stays. Import in OpenUI.":"Switching preserves device routing.\nManage names and keys in OpenUI.");
}
static void devices_page(int sb) {
  char tmp[256];
  const char *label = sb ? "sing-box" : "WireGuard";
  int all = sb ? draft.sb_all : draft.all;
  header("Device routing");
  snprintf(tmp,sizeof(tmp),"%s \xc2\xb7 New devices: %s",label,all?label:"normal network");
  box(16,65,290,20,"small",tmp);
  int enabled = sb ? data.sb_enabled : data.enabled;
  if (stale()) strcpy(tmp, "Status is stale; changes disabled.");
  else if (enabled) snprintf(tmp,sizeof(tmp),"Checked: %s. Others: direct.",label);
  else snprintf(tmp,sizeof(tmp),"Changes take effect when %s is on.",sb?"sing-box":"WG");
  box(16,85,290,20,"small",tmp);
  for(int i=0;i<4&&offset+i<draft.n;i++) {
    struct row *r=&draft.rows[offset+i];
    int on = sb ? r->sb : r->selected;
    int dup=!strcmp(r->name,"-");
    for(int j=0;j<draft.n;j++)if(j!=offset+i&&!strcmp(r->name,draft.rows[j].name))dup=1;
    add("<a id=\"d%d\" href=\"act:d%d\" class=\"box row\" style=\"left:12px;top:%dpx;width:296px\"><div class=\"name\">",i,i,110+i*60);
    add("<span class=\"%s\">%s</span> ",on?"good":"small",on?"[x]":"[ ]");
    shortened(!strcmp(r->name,"-")?"Unknown device":r->name,234);
    add("</div><div class=\"sub\">");
    snprintf(tmp,sizeof(tmp),"%s %s%s%s",r->online?"Online":"Offline",r->ip,dup?" | ":"",dup?r->mac+9:"");
    escaped(tmp);add("</div></a>");
  }
  if(!draft.n)box(24,174,272,48,"small","No devices found");
  linkbox(12,354,96,48,"prev","Previous",offset>0,0);
  linkbox(212,354,96,48,"next","Next",offset+4<draft.n,0);
  snprintf(tmp,sizeof(tmp),"%d / %d",offset/4+1,draft.n?(draft.n+3)/4:1);box(120,368,80,24,"small center",tmp);
  linkbox(12,416,142,52,"back","Cancel",1,0);
  linkbox(166,416,142,52,"save",pending?"Applying":"Save",dirty&&!blocked(),1);
}
static void wireguard_page(void) {
  char tmp[256], rx[32], tx[32];
  header("WireGuard");
  box(12,66,296,112,"card","");
  snprintf(tmp,sizeof(tmp),"title %s",tone_class(wg_tone()));
  box(24,76,272,30,tmp,summary());
  int sb_on = data.sb_ok && data.sb_enabled;
  if(!data.has)snprintf(tmp,sizeof(tmp),"Import a config in OpenUI");
  else if(data.enabled&&data.handshake)snprintf(tmp,sizeof(tmp),"Last handshake: %llu s ago",(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
  else if(sb_on&&!data.enabled)snprintf(tmp,sizeof(tmp),"sing-box is on. Turn it off first.");
  else snprintf(tmp,sizeof(tmp),"%s",data.enabled?"Waiting for the server":"Devices use normal internet");
  box(24,115,272,22,sb_on&&!data.enabled&&data.has?"small warn":"small",tmp);
  box(24,146,272,20,"small","Internet access not verified");
  amount(rx,sizeof(rx),data.rx);amount(tx,sizeof(tx),data.tx);
  box(18,186,140,20,"small","Downloaded");box(174,186,134,20,"small","Uploaded");
  box(18,210,142,28,"value",rx);box(174,210,134,28,"value",tx);
  linkbox(12,248,296,52,"toggle",data.enabled?"Disable WireGuard":"Enable WireGuard",!blocked()&&data.has&&(data.enabled||!sb_on),1);
  snprintf(tmp,sizeof(tmp),"Profile: %s  >",active_profile());
  linkbox(12,312,296,50,"profiles",tmp,1,0);
  snprintf(tmp,sizeof(tmp),"Device routing  %d / %d  >",selected_count(0),data.n);
  linkbox(12,374,296,50,"devices",tmp,1,0);
  box(16,437,294,20,"footer",data.all?"New devices: WireGuard":"New devices: normal network");
  if(notice[0])box(16,457,294,20,"footer bad",notice);
}
static const char *page_title(void) {
  switch (page) {
    case SB: case SB_PROFILES: return "sing-box";
    case SB_NODES: return "Nodes";
    case WG: case WG_PROFILES: return "WireGuard";
    case SB_DEVICES: case WG_DEVICES: return "Device routing";
    default: return "MU5250";
  }
}
static void dialog_page(void) {
  /* A dialog replaces the page: header plus dialog content. */
  header(page_title());
  if (dialog==1 || dialog==4) {
    int sb = dialog_feature;
    box(12,100,296,286,"card","");
    if (dialog==1) {
      box(26,120,268,36,"title",sb?"Disable sing-box?":"Disable WireGuard?");
      box(26,170,268,64,"","Devices will return to the normal network.");
    } else {
      int enabled = sb ? data.sb_enabled : data.enabled;
      box(26,120,268,36,"title","Switch profile?");
      box(26,170,268,64,"",sb?(enabled?"Restarts sing-box. Traffic pauses briefly.":"Select this profile. sing-box will remain off."):
                                (enabled?"Tunnel traffic will briefly pause.":"Select this profile. WireGuard will remain off."));
      add("<div class=\"box small\" style=\"left:26px;top:244px;width:268px;height:40px\">"); shortened(chosen_name,260); add("</div>");
    }
    linkbox(24,302,128,56,"dismiss","Cancel",1,0);
    linkbox(168,302,128,56,"confirm",dialog==1?"Disable":"Switch",!blocked(),1);
  } else if (dialog==2) {
    box(20,110,280,40,"title","Unsaved changes");
    linkbox(24,194,272,56,"saveback","Save and return",!blocked(),1);
    linkbox(24,266,272,56,"discard","Discard changes",1,0);
    linkbox(24,338,272,56,"dismiss","Keep editing",1,0);
  } else {
    box(24,118,272,40,"title bad","Action failed");
    box(24,184,272,72,"",notice);
    box(24,270,272,42,"small","Reload status before trying again.");
    linkbox(24,334,272,56,"refresh","Refresh and back",1,1);
  }
}
static void render(struct drm_buf *b) {
  static int initialized;
  static uint16_t pixels[W * H];
  if (!initialized) {
    html_view_init(pixels,W,H,W,0,NULL);
    initialized=1;
  }
  html_used=0;html[0]=0;
  add("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"UTF-8\"><style>%s</style></head><body>",style_asset);
  if (dialog) dialog_page();
  else if (page==SB) singbox_page();
  else if (page==SB_NODES) nodes_page();
  else if (page==SB_PROFILES) profiles_page(1);
  else if (page==SB_DEVICES) devices_page(1);
  else if (page==WG) wireguard_page();
  else if (page==WG_PROFILES) profiles_page(0);
  else if (page==WG_DEVICES) devices_page(0);
  else overview_page();
  add("</body></html>");
  html_view_render_to(pixels,html);
  for(int y=0;y<H;y++)memcpy((uint8_t*)b->map+y*b->pitch,pixels+y*W,W*2);
}

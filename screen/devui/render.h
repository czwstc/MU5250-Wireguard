/* DevUI HTML/CSS pages rendered by the upstream litehtml host.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static char html[32768];
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
static const char *summary(void) {
  return !data.stamp ? "Loading status" : stale() ? "Status is stale" :
    pending || data.busy ? "Applying changes" : !data.has ? "No configuration" :
    data.error ? "Tunnel error" : !data.enabled ? "Off" :
    data.connected ? "Recent handshake" : "Waiting for handshake";
}
static const char *active_profile(void) {
  for (int i=0;i<data.np;i++) if(data.profiles[i].active) return data.profiles[i].name;
  return "None selected";
}
static int selected_count(void) {
  int n=0;for(int i=0;i<data.n;i++) n+=data.rows[i].selected;return n;
}
static void header(const char *title) {
  box(16,18,188,32,"title",title);
  linkbox(210,6,100,50,"back","Back",1,0);
}
static void battery_badge(int x, int y, int large) {
  int percent = data.battery < 0 ? 0 : data.battery > 100 ? 100 : data.battery;
  const char *color = data.battery < 0 ? "#a7b6ca" : percent <= 20 ? "#ff9999" : "#64e2aa";
  int w=large?62:30, h=large?30:18;
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;border:2px solid %s;border-radius:3px\"></div>",x,y,w,h,color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:4px;height:%dpx;background:%s\"></div>",x+w,y+h/3,h/3,color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;background:%s\"></div>",x+4,y+4,(w-8)*percent/100,h-8,color);
  char label[24];
  if(data.battery<0)snprintf(label,sizeof(label),"--%%");else snprintf(label,sizeof(label),"%d%%",percent);
  box(x+w+10,y-5,large?140:68,large?46:30,large?"battery-value":"title",label);
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
  char tmp[256],rx[32],tx[32];
  if (dialog) {
    header("WireGuard");
    if (dialog==1 || dialog==4) {
      box(12,100,296,286,"card","");
      box(26,120,268,36,"title",dialog==1?"Disable WireGuard?":"Switch profile?");
      box(26,170,268,56,"",dialog==1?"Devices will return to the normal network.":data.enabled?"Tunnel traffic will briefly pause. Routing choices stay saved.":"Select this profile. WireGuard will remain off.");
      if(dialog==4)box(26,238,268,40,"small",chosen_name);
      linkbox(24,302,128,56,"dismiss","Cancel",1,0);
      linkbox(168,302,128,56,"confirm",dialog==1?"Disable":"Switch",!blocked(),1);
    } else if(dialog==2) {
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
  } else if(page==MENU) {
    add("%s",menu_asset);
    battery_badge(196,68,0);
    const int pages[] = {OVERVIEW,WG,PROFILES,DEVICES};
    const char *ids[] = {"overview","wireguard","profiles","devices"};
    const char *titles[] = {"Overview","WireGuard","Saved profiles","Device routing"};
    const char *details[] = {"Battery, uptime and system load","Tunnel status and on / off","Switch between up to 5 configs","Choose WireGuard or direct"};
    int row=0;
    for(int i=0;i<4;i++) if(visible_page(pages[i])) {
      add("<a id=\"%s\" href=\"act:%s\" class=\"box row\" style=\"left:12px;top:%dpx;width:296px;height:64px\"><div class=\"name\">%s</div><div class=\"sub\">%s</div></a>",ids[i],ids[i],104+76*row++,titles[i],details[i]);
    }
    snprintf(tmp,sizeof(tmp),"Short power press: sleep / wake.\nStock UI after %d seconds idle.",settings_idle);
    box(16,432,294,36,"footer",tmp);
  } else if(page==WG) {
    header("WireGuard");
    box(12,66,296,112,"card","");
    box(24,76,272,30,data.error||stale()?"title bad":data.enabled&&data.connected?"title good":"title",summary());
    if(!data.has)snprintf(tmp,sizeof(tmp),"Import a config in OpenUI");
    else if(data.enabled&&data.handshake)snprintf(tmp,sizeof(tmp),"Last handshake: %llu s ago",(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
    else snprintf(tmp,sizeof(tmp),"%s",data.enabled?"Waiting for the server":"Devices use normal internet");
    box(24,115,272,22,"small",tmp);
    box(24,146,272,20,"small","Internet access not verified");
    amount(rx,sizeof(rx),data.rx);amount(tx,sizeof(tx),data.tx);
    box(18,186,140,20,"small","Downloaded");box(174,186,134,20,"small","Uploaded");
    box(18,210,142,28,"value",rx);box(174,210,134,28,"value",tx);
    linkbox(12,248,296,52,"toggle",data.enabled?"Disable WireGuard":"Enable WireGuard",!blocked()&&data.has,1);
    snprintf(tmp,sizeof(tmp),"Profile: %s",active_profile());
    linkbox(12,312,296,50,"profiles",tmp,visible_page(PROFILES),0);
    snprintf(tmp,sizeof(tmp),"Device routing    %d / %d  >",selected_count(),data.n);
    linkbox(12,374,296,50,"devices",tmp,visible_page(DEVICES),0);
    box(16,437,294,20,"footer",data.all?"New devices: WireGuard":"New devices: normal network");
    if(notice[0])box(16,457,294,20,"footer bad",notice);
  } else if(page==PROFILES) {
    header("Saved profiles");
    snprintf(tmp,sizeof(tmp),"%d / 5 saved - select to switch",data.np);
    box(16,66,290,24,"small",tmp);
    for(int i=0;i<data.np;i++) {
      char id[16];snprintf(id,sizeof(id),"p%d",i);
      snprintf(tmp,sizeof(tmp),"%s%s",data.profiles[i].active?"[Active] ":"",data.profiles[i].name);
      linkbox(12,106+i*60,296,52,id,tmp,!blocked()&&!data.profiles[i].active,data.profiles[i].active);
    }
    if(!data.np)box(24,144,272,100,"","Import profiles in the OpenUI webpage first.");
    box(16,420,290,48,"footer",stale()?"Status is stale. Controls are disabled.":"Switching preserves device routing.\nManage names and keys in OpenUI.");
  } else if(page==DEVICES) {
    header("Device routing");
    box(16,65,290,20,"small",draft.all?"New devices: WireGuard":"New devices: normal network");
    box(16,85,290,20,"small",stale()?"Status is stale; changes disabled.":data.enabled?"Checked: WireGuard. Others: direct.":"Changes take effect when WG is on.");
    for(int i=0;i<4&&offset+i<draft.n;i++) {
      struct row *r=&draft.rows[offset+i];
      int dup=!strcmp(r->name,"-");
      for(int j=0;j<draft.n;j++)if(j!=offset+i&&!strcmp(r->name,draft.rows[j].name))dup=1;
      add("<a id=\"d%d\" href=\"act:d%d\" class=\"box row\" style=\"left:12px;top:%dpx;width:296px\"><div class=\"name\">",i,i,110+i*60);
      add("<span class=\"%s\">%s</span> ",r->selected?"good":"small",r->selected?"[x]":"[ ]");
      shortened(!strcmp(r->name,"-")?"Unknown device":r->name,234);
      add("</div><div class=\"sub\">");
      snprintf(tmp,sizeof(tmp),"%s %s%s%s",r->online?"Online":"Offline",r->ip,dup?" | ":"",dup?r->mac+9:"");
      escaped(tmp);add("</div></a>");
    }
    if(!draft.n)box(24,174,272,48,"small","No devices found");
    linkbox(12,354,96,48,"prev","Previous",offset>0,0);
    linkbox(212,354,96,48,"next","Next",offset+4<draft.n,0);
    snprintf(tmp,sizeof(tmp),"%d / %d",offset/4+1,draft.n?(draft.n+3)/4:1);box(120,368,80,24,"small",tmp);
    linkbox(12,416,142,52,"back","Cancel",1,0);
    linkbox(166,416,142,52,"save",pending?"Applying":"Save",dirty&&!blocked(),1);
  } else if(page==OVERVIEW) {
    header("Overview");
    box(12,66,296,92,"card","");
    box(24,72,160,20,"small","BATTERY REMAINING");
    battery_badge(24,110,1);
    if(data.temperature!=-999)snprintf(tmp,sizeof(tmp),"Battery temp       %.1f C",data.temperature/10.0);else strcpy(tmp,"Battery temp          --");
    box(24,172,272,26,"",tmp);
    snprintf(tmp,sizeof(tmp),"Uptime        %lluh %llum",data.uptime/3600,data.uptime/60%60);box(24,208,272,26,"",tmp);
    snprintf(tmp,sizeof(tmp),"CPU                %.1f%%",data.cpu);box(24,244,272,26,"",tmp);
    if(data.memory>=0)snprintf(tmp,sizeof(tmp),"Memory             %.1f%%",data.memory);else strcpy(tmp,"Memory                --");box(24,280,272,26,"",tmp);
    box(16,320,290,28,stale()?"small bad":"small",stale()?"Status unavailable / stale":"Live status from OpenUI agent");
    snprintf(tmp,sizeof(tmp),"WireGuard: %s",summary());box(16,348,290,32,"",tmp);
    linkbox(12,406,296,54,"wireguard","Open WireGuard",visible_page(WG),1);
  }
  add("</body></html>");
  html_view_render_to(pixels,html);
  for(int y=0;y<H;y++)memcpy((uint8_t*)b->map+y*b->pitch,pixels+y*W,W*2);
}

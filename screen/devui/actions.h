/* Whitelisted DevUI actions. No scripts, passwords or keys in the UI.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static int sb_page(int p) { return p == SB || p == SB_NODES || p == SB_PROFILES || p == SB_DEVICES; }
static int parent_of(int p) {
  return p == SB_NODES || p == SB_PROFILES || p == SB_DEVICES ? SB :
         p == WG_PROFILES || p == WG_DEVICES ? WG : OVERVIEW;
}
static void go(int p) {
  page = visible_page(p) ? p : OVERVIEW;
  offset = 0; node_offset = 0;
}
static void back(void) {
  if(dirty&&!pending)dialog=2;
  else { go(parent_of(page));dirty=0;dialog=0; }
  need_draw=1;
}
static void enter_devices(int target) {
  int sb = target == SB_DEVICES;
  draft=data;
  for(int i=0;i<draft.n;i++)original_selection[i]=sb?draft.rows[i].sb:draft.rows[i].selected;
  offset=0;dirty=0;page=target;
}
static void tap(int x,int y) {
  char action[96];
  snprintf(action,sizeof(action),"%s",html_view_click((float)x,(float)y));
  if(strncmp(action,"act:",4))return;
  const char *a=action+4;
  if(!strcmp(a,"none"))return;
  need_draw=1;
  int sb = sb_page(page);
  if(dialog) {
    if(!strcmp(a,"dismiss"))dialog=0;
    else if(!strcmp(a,"confirm")&&!blocked()) {
      if(dialog==1)submit(A_TOGGLE,dialog_feature,0);
      else if(dialog==4)submit(A_PROFILE,dialog_feature,0);
    } else if(!strcmp(a,"saveback")&&dialog==2&&!blocked()) {
      return_after_save=1;submit(A_DEVICES,sb,0);
    } else if(!strcmp(a,"discard")&&dialog==2) {
      dirty=0;go(parent_of(page));dialog=0;
    } else if(!strcmp(a,"refresh")&&dialog==3) {
      dirty=0;go(sb?SB:WG);dialog=0;notice[0]=0;
    } else if(!strcmp(a,"back"))dialog=0;
    return;
  }
  if(!strcmp(a,"stock")) { quitting=1;return; }
  if(!strcmp(a,"back")) { back();return; }
  if(page==OVERVIEW) {
    if(!strcmp(a,"singbox")&&visible_page(SB))go(SB);
    else if(!strcmp(a,"wireguard")&&visible_page(WG))go(WG);
    return;
  }
  if(page==SB) {
    if(!strcmp(a,"sbtoggle")&&!blocked()) {
      if(data.sb_enabled) { dialog=1;dialog_feature=1; }
      else submit(A_TOGGLE,1,1);
    } else if(!strcmp(a,"nodes")&&data.nn>0) { page=SB_NODES;node_offset=data.node_now>=4?data.node_now/4*4:0; }
    else if(!strcmp(a,"sbprofiles"))page=SB_PROFILES;
    else if(!strcmp(a,"sbdevices"))enter_devices(SB_DEVICES);
    return;
  }
  if(page==WG) {
    if(!strcmp(a,"toggle")&&!blocked()&&data.has) {
      if(data.enabled) { dialog=1;dialog_feature=0; }
      else submit(A_TOGGLE,0,1);
    } else if(!strcmp(a,"profiles"))page=WG_PROFILES;
    else if(!strcmp(a,"devices"))enter_devices(WG_DEVICES);
    return;
  }
  if(page==SB_NODES) {
    if(!strcmp(a,"nprev")&&node_offset>0)node_offset-=4;
    else if(!strcmp(a,"nnext")&&node_offset+4<data.nn)node_offset+=4;
    else if(!strcmp(a,"test")&&!blocked())submit(A_DELAY,1,0);
    else if(a[0]=='n'&&a[1]>='0'&&a[1]<='3'&&!a[2]&&!blocked()) {
      int i=node_offset+a[1]-'0';
      if(i<data.nn&&i!=data.node_now)submit(A_NODE,1,i);
    }
    return;
  }
  if((page==SB_PROFILES||page==WG_PROFILES)&&a[0]=='p'&&a[1]>='0'&&a[1]<='4'&&!a[2]&&!blocked()) {
    int i=a[1]-'0', count=sb?data.nq:data.np;
    const struct profile *p=sb?&data.sb_profiles[i]:&data.profiles[i];
    if(i<count&&!p->active) {
      chosen_profile=p->id;chosen_revision=sb?data.sb_rev:data.rev;
      snprintf(chosen_name,sizeof(chosen_name),"%s",p->name);
      dialog=4;dialog_feature=sb;
    }
    return;
  }
  if(page==SB_DEVICES||page==WG_DEVICES) {
    if(!strcmp(a,"prev")&&offset>0)offset-=4;
    else if(!strcmp(a,"next")&&offset+4<draft.n)offset+=4;
    else if(!strcmp(a,"save")&&dirty&&!blocked()) {
      return_after_save=0;submit(A_DEVICES,sb,0);
    } else if(a[0]=='d'&&a[1]>='0'&&a[1]<='3'&&!a[2]&&!blocked()) {
      int i=offset+a[1]-'0';
      if(i<draft.n) {
        int *on=sb?&draft.rows[i].sb:&draft.rows[i].selected;
        *on=!*on;
        dirty=0;
        for(int j=0;j<draft.n;j++)if((sb?draft.rows[j].sb:draft.rows[j].selected)!=original_selection[j])dirty=1;
      }
    }
  }
}

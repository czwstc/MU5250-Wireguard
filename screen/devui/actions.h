/* Whitelisted DevUI actions. No scripts, passwords or keys in the UI.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static void back(void) {
  if(dirty&&!pending)dialog=2;
  else { page=page==WG||page==OVERVIEW?MENU:WG;dirty=0;dialog=0; }
  need_draw=1;
}
static void enter_devices(void) {
  draft=data;
  for(int i=0;i<draft.n;i++)original_selection[i]=draft.rows[i].selected;
  offset=0;dirty=0;page=DEVICES;
}
static void tap(int x,int y) {
  char action[96];
  snprintf(action,sizeof(action),"%s",html_view_click((float)x,(float)y));
  if(strncmp(action,"act:",4))return;
  const char *a=action+4;
  if(!strcmp(a,"none"))return;
  need_draw=1;
  if(dialog) {
    if(!strcmp(a,"dismiss"))dialog=0;
    else if(!strcmp(a,"confirm")&&!blocked()) {
      if(dialog==1)submit(1,0);
      else if(dialog==4)submit(2,0);
    } else if(!strcmp(a,"saveback")&&dialog==2&&!blocked()) {
      return_after_save=1;submit(0,0);
    } else if(!strcmp(a,"discard")&&dialog==2) {
      dirty=0;page=WG;dialog=0;
    } else if(!strcmp(a,"refresh")&&dialog==3) {
      dirty=0;page=WG;dialog=0;notice[0]=0;
    } else if(!strcmp(a,"back"))dialog=0;
    return;
  }
  if(!strcmp(a,"stock")) { quitting=1;return; }
  if(!strcmp(a,"back")) { back();return; }
  if(!strcmp(a,"wireguard")) { page=WG;return; }
  if(!strcmp(a,"overview")) { page=OVERVIEW;return; }
  if(!strcmp(a,"profiles")) { page=PROFILES;return; }
  if(!strcmp(a,"devices")) { enter_devices();return; }
  if(!strcmp(a,"toggle")&&page==WG&&!blocked()&&data.has) {
    if(data.enabled)dialog=1;else submit(1,1);
  } else if(page==PROFILES&&a[0]=='p'&&a[1]>='0'&&a[1]<='4'&&!a[2]&&!blocked()) {
    int i=a[1]-'0';
    if(i<data.np&&!data.profiles[i].active) {
      chosen_profile=data.profiles[i].id;chosen_revision=data.rev;
      snprintf(chosen_name,sizeof(chosen_name),"%s",data.profiles[i].name);
      dialog=4;
    }
  } else if(page==DEVICES) {
    if(!strcmp(a,"prev")&&offset>0)offset-=4;
    else if(!strcmp(a,"next")&&offset+4<draft.n)offset+=4;
    else if(!strcmp(a,"save")&&dirty&&!blocked()) {
      return_after_save=0;submit(0,0);
    } else if(a[0]=='d'&&a[1]>='0'&&a[1]<='3'&&!a[2]&&!blocked()) {
      int i=offset+a[1]-'0';
      if(i<draft.n) {
        draft.rows[i].selected=!draft.rows[i].selected;
        dirty=0;
        for(int j=0;j<draft.n;j++)if(draft.rows[j].selected!=original_selection[j])dirty=1;
      }
    }
  }
}

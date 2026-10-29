/* A WPEPlatform for the HTML preview (`html_wpe.rs`): WebKit renders each frame into a shared
 * memory buffer handed to the host as straight-through BGRA, and input reaches the page as real
 * WPEEvents (trusted, with user activation) rather than script-dispatched DOM events.
 *
 * Built as its own shared object and loaded with RTLD_DEEPBIND: the process also links
 * WebKitGTK, which exports the same `webkit_*` and `jsc_*` symbols. Every function runs on the
 * thread that owns the default GLib main context (`recur-wpe`).
 *
 * The display reports no DRM device, so WebKit never offers DMA-BUF and renders to SHM.
 */
#include <string.h>
#include <wpe/webkit.h>
#include <wpe/wpe-platform.h>

typedef void (*RecurWpeEvent)(guintptr ctx, int kind, const char *text, int width, int height,
                              int stride, const guint8 *pixels);

enum { EV_FRAME, EV_MESSAGE, EV_LOADED, EV_CURSOR, EV_REPLY };

typedef struct {
    WebKitWebView *web;
    WPEView *view;
    RecurWpeEvent cb;
    guintptr ctx;
    WebKitOptionMenu *menu;
    gboolean refused;
} RecurWpePage;

/* Display */

struct _RecurDisplay {
    WPEDisplay parent;
    WPEKeymap *keymap;
};
G_DECLARE_FINAL_TYPE(RecurDisplay, recur_display, RECUR, DISPLAY, WPEDisplay)

struct _RecurToplevel {
    WPEToplevel parent;
};
G_DECLARE_FINAL_TYPE(RecurToplevel, recur_toplevel, RECUR, TOPLEVEL, WPEToplevel)

struct _RecurView {
    WPEView parent;
    WPEBuffer *pending, *committed;
    GSource *frame;
    gint64 last_frame;
};
G_DECLARE_FINAL_TYPE(RecurView, recur_view, RECUR, VIEW, WPEView)

struct _RecurIM {
    WPEInputMethodContext parent;
    char *preedit;
    guint cursor;
};
G_DECLARE_FINAL_TYPE(RecurIM, recur_im, RECUR, IM, WPEInputMethodContext)

G_DEFINE_FINAL_TYPE(RecurDisplay, recur_display, WPE_TYPE_DISPLAY)
G_DEFINE_FINAL_TYPE(RecurToplevel, recur_toplevel, WPE_TYPE_TOPLEVEL)
G_DEFINE_FINAL_TYPE(RecurView, recur_view, WPE_TYPE_VIEW)
G_DEFINE_FINAL_TYPE(RecurIM, recur_im, WPE_TYPE_INPUT_METHOD_CONTEXT)

static gboolean display_connect(WPEDisplay *d, GError **e) { return TRUE; }

static WPEView *display_create_view(WPEDisplay *d)
{
    return WPE_VIEW(g_object_new(recur_view_get_type(), "display", d, NULL));
}

static WPEToplevel *display_create_toplevel(WPEDisplay *d, guint max_views)
{
    return WPE_TOPLEVEL(g_object_new(recur_toplevel_get_type(), "display", d, NULL));
}

static WPEKeymap *display_get_keymap(WPEDisplay *d)
{
    RecurDisplay *self = RECUR_DISPLAY(d);
    if (!self->keymap)
        self->keymap = wpe_keymap_xkb_new();
    return self->keymap;
}

static WPEInputMethodContext *display_create_im(WPEDisplay *d, WPEView *view)
{
    WPEInputMethodContext *im = g_object_new(recur_im_get_type(), "view", view, NULL);
    g_object_set_data(G_OBJECT(view), "recur-im", im);
    return im;
}

static void recur_display_class_init(RecurDisplayClass *klass)
{
    WPEDisplayClass *c = WPE_DISPLAY_CLASS(klass);
    c->connect = display_connect;
    c->create_view = display_create_view;
    c->create_toplevel = display_create_toplevel;
    c->get_keymap = display_get_keymap;
    c->create_input_method_context = display_create_im;
}
static void recur_display_init(RecurDisplay *d) {}

/* Toplevel: the preview's box. */

static void toplevel_constructed(GObject *o)
{
    G_OBJECT_CLASS(recur_toplevel_parent_class)->constructed(o);
    wpe_toplevel_state_changed(WPE_TOPLEVEL(o), WPE_TOPLEVEL_STATE_ACTIVE);
}

static gboolean resize_view(WPEToplevel *t, WPEView *view, gpointer data)
{
    int w, h;
    wpe_toplevel_get_size(t, &w, &h);
    wpe_view_resized(view, w, h);
    return FALSE;
}

static gboolean toplevel_resize(WPEToplevel *t, int w, int h)
{
    wpe_toplevel_resized(t, w, h);
    wpe_toplevel_foreach_view(t, resize_view, NULL);
    return TRUE;
}

static void recur_toplevel_class_init(RecurToplevelClass *klass)
{
    G_OBJECT_CLASS(klass)->constructed = toplevel_constructed;
    WPE_TOPLEVEL_CLASS(klass)->resize = toplevel_resize;
}
static void recur_toplevel_init(RecurToplevel *t) {}

/* View: frames go to the host, and `buffer_rendered` (WebKit's frame callback) to WebKit, on a
 * 60 Hz tick, as the headless platform paces them. */

static gboolean view_frame_dispatch(GSource *source, GSourceFunc cb, gpointer data)
{
    if (g_source_get_ready_time(source) == -1)
        return G_SOURCE_CONTINUE;
    g_source_set_ready_time(source, -1);
    return cb(data);
}

static GSourceFuncs frame_funcs = { NULL, NULL, view_frame_dispatch, NULL, NULL, NULL };

/* Hands the buffer's pixels to the host. */
static gboolean deliver(RecurView *self, WPEBuffer *buffer, GError **error)
{
    RecurWpePage *page = g_object_get_data(G_OBJECT(self), "recur-page");
    if (!page)
        return TRUE;
    int w = wpe_buffer_get_width(buffer), h = wpe_buffer_get_height(buffer), stride;
    GBytes *bytes;
    if (WPE_IS_BUFFER_SHM(buffer)) {
        bytes = wpe_buffer_shm_get_data(WPE_BUFFER_SHM(buffer));
        stride = wpe_buffer_shm_get_stride(WPE_BUFFER_SHM(buffer));
    } else {
        bytes = wpe_buffer_import_to_pixels(buffer, error);
        stride = w * 4;
    }
    if (!bytes)
        return FALSE;
    page->cb(page->ctx, EV_FRAME, NULL, w, h, stride, g_bytes_get_data(bytes, NULL));
    return TRUE;
}

/* The 60 Hz tick: the latest buffer goes to the host and WebKit may render the next. */
static gboolean view_frame(gpointer data)
{
    RecurView *self = data;
    self->last_frame = g_get_monotonic_time();
    if (self->committed)
        wpe_view_buffer_released(WPE_VIEW(self), self->committed);
    g_clear_object(&self->committed);
    self->committed = g_steal_pointer(&self->pending);
    if (self->committed) {
        deliver(self, self->committed, NULL);
        wpe_view_buffer_rendered(WPE_VIEW(self), self->committed);
    }
    return G_SOURCE_CONTINUE;
}

static void view_mapped_on_toplevel(WPEView *view, GParamSpec *p, gpointer data)
{
    WPEToplevel *t = wpe_view_get_toplevel(view);
    if (!t) {
        wpe_view_unmap(view);
        return;
    }
    int w, h;
    wpe_toplevel_get_size(t, &w, &h);
    if (w && h)
        wpe_view_resized(view, w, h);
    wpe_view_map(view);
}

static void view_constructed(GObject *o)
{
    G_OBJECT_CLASS(recur_view_parent_class)->constructed(o);
    RecurView *self = RECUR_VIEW(o);
    g_signal_connect(o, "notify::toplevel", G_CALLBACK(view_mapped_on_toplevel), NULL);
    self->frame = g_source_new(&frame_funcs, sizeof(GSource));
    g_source_set_callback(self->frame, view_frame, self, NULL);
    g_source_set_ready_time(self->frame, -1);
    g_source_attach(self->frame, g_main_context_get_thread_default());
}

static void view_dispose(GObject *o)
{
    RecurView *self = RECUR_VIEW(o);
    if (self->frame) {
        g_source_destroy(self->frame);
        g_clear_pointer(&self->frame, g_source_unref);
    }
    g_clear_object(&self->pending);
    g_clear_object(&self->committed);
    G_OBJECT_CLASS(recur_view_parent_class)->dispose(o);
}

static gboolean view_render_buffer(WPEView *view, WPEBuffer *buffer, const WPERectangle *damage,
                                   guint n_damage, GError **error)
{
    RecurView *self = RECUR_VIEW(view);
    if (self->pending)
        wpe_view_buffer_released(view, self->pending);
    g_set_object(&self->pending, buffer);
    gint64 now = g_get_monotonic_time(), next = self->last_frame + G_USEC_PER_SEC / 60;
    g_source_set_ready_time(self->frame, next <= now ? 0 : next);
    return TRUE;
}

static void view_set_cursor(WPEView *view, const char *name)
{
    RecurWpePage *page = g_object_get_data(G_OBJECT(view), "recur-page");
    if (page)
        page->cb(page->ctx, EV_CURSOR, name, 0, 0, 0, NULL);
}

static void recur_view_class_init(RecurViewClass *klass)
{
    G_OBJECT_CLASS(klass)->constructed = view_constructed;
    G_OBJECT_CLASS(klass)->dispose = view_dispose;
    WPE_VIEW_CLASS(klass)->render_buffer = view_render_buffer;
    WPE_VIEW_CLASS(klass)->set_cursor_from_name = view_set_cursor;
}
static void recur_view_init(RecurView *v) {}

/* Input method: the host's IME (GPUI's input handler) pushes the preedit and commits. */

static void im_get_preedit(WPEInputMethodContext *c, gchar **text, GList **underlines, guint *cursor)
{
    RecurIM *self = RECUR_IM(c);
    const char *s = self->preedit ? self->preedit : "";
    if (text)
        *text = g_strdup(s);
    if (underlines)
        *underlines = *s ? g_list_prepend(NULL, wpe_input_method_underline_new(0, g_utf8_strlen(s, -1))) : NULL;
    if (cursor)
        *cursor = self->cursor;
}

static gboolean im_filter(WPEInputMethodContext *c, WPEEvent *e) { return FALSE; }

static void im_finalize(GObject *o)
{
    g_free(RECUR_IM(o)->preedit);
    G_OBJECT_CLASS(recur_im_parent_class)->finalize(o);
}

static void recur_im_class_init(RecurIMClass *klass)
{
    G_OBJECT_CLASS(klass)->finalize = im_finalize;
    WPE_INPUT_METHOD_CONTEXT_CLASS(klass)->get_preedit_string = im_get_preedit;
    WPE_INPUT_METHOD_CONTEXT_CLASS(klass)->filter_key_event = im_filter;
}
static void recur_im_init(RecurIM *c) {}

/* Page */

static WPEDisplay *display(void)
{
    static WPEDisplay *d;
    if (!d) {
        d = g_object_new(recur_display_get_type(), NULL);
        wpe_display_connect(d, NULL);
        /* WebKit asks the primary display for a DRM device; without this it probes the
         * built-in headless one, which finds /dev/dri and turns on DMA-BUF. */
        wpe_display_set_primary(d);
    }
    return d;
}

static WebKitWebView *on_create_view(WebKitWebView *web, WebKitNavigationAction *a, gpointer data)
{
    return NULL;
}

/* Navigations to https (iframes) and about: are allowed; the main frame's response is refused
 * unless it is about:blank, so the document never navigates away. */
static gboolean on_decide_policy(WebKitWebView *web, WebKitPolicyDecision *d,
                                 WebKitPolicyDecisionType type, gpointer data)
{
    RecurWpePage *page = data;
    if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        webkit_policy_decision_ignore(d);
        return TRUE;
    }
    if (type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
        WebKitNavigationAction *a = webkit_navigation_policy_decision_get_navigation_action(
            WEBKIT_NAVIGATION_POLICY_DECISION(d));
        const char *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(a));
        if (g_str_has_prefix(uri, "https:") || g_str_has_prefix(uri, "about:"))
            webkit_policy_decision_use(d);
        else
            webkit_policy_decision_ignore(d);
        return TRUE;
    }
    if (type == WEBKIT_POLICY_DECISION_TYPE_RESPONSE) {
        WebKitResponsePolicyDecision *r = WEBKIT_RESPONSE_POLICY_DECISION(d);
        const char *uri = webkit_uri_request_get_uri(webkit_response_policy_decision_get_request(r));
        if (webkit_response_policy_decision_is_main_frame_main_resource(r) && strcmp(uri, "about:blank")) {
            page->refused = TRUE;
            webkit_policy_decision_ignore(d);
            return TRUE;
        }
    }
    return FALSE;
}

static void on_load_changed(WebKitWebView *web, WebKitLoadEvent ev, gpointer data)
{
    RecurWpePage *page = data;
    if (ev != WEBKIT_LOAD_FINISHED)
        return;
    if (page->refused)
        page->refused = FALSE;
    else
        page->cb(page->ctx, EV_LOADED, NULL, 0, 0, 0, NULL);
}

static void on_script_message(WebKitUserContentManager *m, JSCValue *v, gpointer data)
{
    RecurWpePage *page = data;
    char *s = jsc_value_to_string(v);
    page->cb(page->ctx, EV_MESSAGE, s, 0, 0, 0, NULL);
    g_free(s);
}

/* A `<select>`'s popup: its box and options go to the host as `{select: ...}`, as
 * `html_gtk::INPUT_JS` answers, and `recur_wpe_page_pick` closes it. */
static gboolean on_show_option_menu(WebKitWebView *web, WebKitOptionMenu *menu, WebKitRectangle *r,
                                    gpointer data)
{
    RecurWpePage *page = data;
    g_set_object(&page->menu, menu);
    GString *s = g_string_new(NULL);
    g_string_append_printf(s, "{\"select\":{\"x\":%d,\"y\":%d,\"w\":%d,\"h\":%d,\"options\":[",
                           r->x, r->y, r->width, r->height);
    int index = -1;
    for (guint i = 0; i < webkit_option_menu_get_n_items(menu); i++) {
        WebKitOptionMenuItem *item = webkit_option_menu_get_item(menu, i);
        if (webkit_option_menu_item_is_selected(item))
            index = i;
        char *label = g_strescape(webkit_option_menu_item_get_label(item), NULL);
        g_string_append_printf(s, "%s{\"label\":\"%s\",\"disabled\":%s}", i ? "," : "", label,
                               webkit_option_menu_item_is_enabled(item) ? "false" : "true");
        g_free(label);
    }
    g_string_append_printf(s, "],\"index\":%d}}", index);
    page->cb(page->ctx, EV_REPLY, s->str, 0, 0, 0, NULL);
    g_string_free(s, TRUE);
    return TRUE;
}

RecurWpePage *recur_wpe_page_new(const char *const *scripts, RecurWpeEvent cb, guintptr ctx)
{
    RecurWpePage *page = g_new0(RecurWpePage, 1);
    page->cb = cb;
    page->ctx = ctx;
    WebKitUserContentManager *ucm = webkit_user_content_manager_new();
    webkit_user_content_manager_register_script_message_handler(ucm, "recur", NULL);
    g_signal_connect(ucm, "script-message-received::recur", G_CALLBACK(on_script_message), page);
    for (; scripts && *scripts; scripts++) {
        WebKitUserScript *us = webkit_user_script_new(*scripts, WEBKIT_USER_CONTENT_INJECT_TOP_FRAME,
                                                      WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
        webkit_user_content_manager_add_script(ucm, us);
        webkit_user_script_unref(us);
    }
    WebKitNetworkSession *session = webkit_network_session_new_ephemeral();
    WebKitSettings *settings = webkit_settings_new();
    webkit_settings_set_javascript_can_open_windows_automatically(settings, FALSE);
    webkit_settings_set_allow_file_access_from_file_urls(settings, FALSE);
    webkit_settings_set_allow_universal_access_from_file_urls(settings, FALSE);
    page->web = g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", display(), "network-session", session,
                             "user-content-manager", ucm, "settings", settings, NULL);
    g_object_unref(ucm);
    g_object_unref(session);
    g_object_unref(settings);
    page->view = webkit_web_view_get_wpe_view(page->web);
    g_object_set_data(G_OBJECT(page->view), "recur-page", page);
    g_signal_connect(page->web, "create", G_CALLBACK(on_create_view), page);
    g_signal_connect(page->web, "decide-policy", G_CALLBACK(on_decide_policy), page);
    g_signal_connect(page->web, "load-changed", G_CALLBACK(on_load_changed), page);
    g_signal_connect(page->web, "show-option-menu", G_CALLBACK(on_show_option_menu), page);
    return page;
}

void recur_wpe_page_free(RecurWpePage *page)
{
    g_object_set_data(G_OBJECT(page->view), "recur-page", NULL);
    g_clear_object(&page->menu);
    g_signal_handlers_disconnect_by_data(page->web, page);
    g_signal_handlers_disconnect_by_data(webkit_web_view_get_user_content_manager(page->web), page);
    g_object_unref(page->web);
    g_free(page);
}

void recur_wpe_page_load(RecurWpePage *page, const char *html)
{
    webkit_web_view_load_html(page->web, html, NULL);
}

/* `w` x `h` CSS pixels at `scale` device pixels each. */
void recur_wpe_page_resize(RecurWpePage *page, int w, int h, double scale)
{
    WPEToplevel *t = wpe_view_get_toplevel(page->view);
    wpe_toplevel_scale_changed(t, scale);
    wpe_toplevel_resize(t, w, h);
}

/* Unmapped while the preview is parked: WebKit stops rendering it. */
void recur_wpe_page_set_active(RecurWpePage *page, gboolean active)
{
    wpe_view_set_visible(page->view, active);
}

void recur_wpe_page_focus(RecurWpePage *page, gboolean focus)
{
    if (focus)
        wpe_view_focus_in(page->view);
    else
        wpe_view_focus_out(page->view);
}

static guint32 now_ms(void) { return g_get_monotonic_time() / 1000; }

/* `type` is a WPEEventType: POINTER_DOWN / UP (with `button`, `count`), MOVE, ENTER, LEAVE. */
void recur_wpe_page_pointer(RecurWpePage *page, int type, double x, double y, guint button,
                            guint count, guint mods)
{
    WPEEvent *e = type == WPE_EVENT_POINTER_DOWN || type == WPE_EVENT_POINTER_UP
        ? wpe_event_pointer_button_new(type, page->view, WPE_INPUT_SOURCE_MOUSE, now_ms(), mods, button, x, y,
                                       type == WPE_EVENT_POINTER_DOWN ? count : 0)
        : wpe_event_pointer_move_new(type, page->view, WPE_INPUT_SOURCE_MOUSE, now_ms(), mods, x, y, 0, 0);
    wpe_view_event(page->view, e);
    wpe_event_unref(e);
}

/* `dx`, `dy` in CSS pixels, positive scrolling the content up / left as in WebKit.
 * Precise mouse deltas are scaled by 2.5 (WebEventFactoryWPE), touchscreen ones are not. */
void recur_wpe_page_scroll(RecurWpePage *page, double x, double y, double dx, double dy, guint mods)
{
    WPEEvent *e = wpe_event_scroll_new(page->view, WPE_INPUT_SOURCE_TOUCHSCREEN, now_ms(), mods,
                                       dx, dy, TRUE, FALSE, x, y);
    wpe_view_event(page->view, e);
    wpe_event_unref(e);
}

static const struct { const char *key; guint keyval; } named_keys[] = {
    { "Enter", WPE_KEY_Return }, { "Backspace", WPE_KEY_BackSpace }, { "Delete", WPE_KEY_Delete },
    { "Tab", WPE_KEY_Tab }, { "Escape", WPE_KEY_Escape }, { "ArrowUp", WPE_KEY_Up },
    { "ArrowDown", WPE_KEY_Down }, { "ArrowLeft", WPE_KEY_Left }, { "ArrowRight", WPE_KEY_Right },
    { "PageUp", WPE_KEY_Page_Up }, { "PageDown", WPE_KEY_Page_Down }, { "Home", WPE_KEY_Home },
    { "End", WPE_KEY_End }, { "Shift", WPE_KEY_Shift_L }, { "Control", WPE_KEY_Control_L },
    { "Alt", WPE_KEY_Alt_L }, { "Meta", WPE_KEY_Super_L },
};

/* A key press (down and up) by its DOM `key` name: a single character or a named key. The
 * hardware keycode comes from the XKB keymap (us layout), for `KeyboardEvent.code`. */
gboolean recur_wpe_page_key(RecurWpePage *page, const char *key, guint mods)
{
    guint keyval = 0;
    for (gsize i = 0; i < G_N_ELEMENTS(named_keys); i++)
        if (!strcmp(key, named_keys[i].key))
            keyval = named_keys[i].keyval;
    if (!keyval && g_utf8_strlen(key, -1) == 1)
        keyval = wpe_unicode_to_keyval(g_utf8_get_char(key));
    if (!keyval)
        return FALSE;
    guint keycode = 0;
    WPEKeymapEntry *entries = NULL;
    guint n = 0;
    if (wpe_keymap_get_entries_for_keyval(wpe_display_get_keymap(display()), keyval, &entries, &n) && n)
        keycode = entries[0].keycode;
    g_free(entries);
    for (int type = WPE_EVENT_KEYBOARD_KEY_DOWN; type <= WPE_EVENT_KEYBOARD_KEY_UP; type++) {
        WPEEvent *e = wpe_event_keyboard_new(type, page->view, WPE_INPUT_SOURCE_KEYBOARD, now_ms(), mods, keycode, keyval);
        wpe_view_event(page->view, e);
        wpe_event_unref(e);
    }
    return TRUE;
}

/* The IME's composition: a non-empty `text` is the preedit (cursor at `cursor` characters),
 * `commit` inserts `text` and ends it. Also how pasted text goes in. */
void recur_wpe_page_ime(RecurWpePage *page, const char *text, guint cursor, gboolean commit)
{
    RecurIM *im = g_object_get_data(G_OBJECT(page->view), "recur-im");
    if (!im)
        return;
    gboolean composing = im->preedit && *im->preedit;
    g_free(im->preedit);
    im->preedit = commit ? NULL : g_strdup(text);
    im->cursor = cursor;
    if (commit) {
        g_signal_emit_by_name(im, "committed", text);
        if (composing)
            g_signal_emit_by_name(im, "preedit-finished");
        return;
    }
    if (!composing && *text)
        g_signal_emit_by_name(im, "preedit-started");
    g_signal_emit_by_name(im, "preedit-changed");
    if (composing && !*text)
        g_signal_emit_by_name(im, "preedit-finished");
}

void recur_wpe_page_command(RecurWpePage *page, const char *command)
{
    webkit_web_view_execute_editing_command(page->web, command);
}

/* Closes the `<select>` popup, choosing `index` (-1 keeps the value). */
void recur_wpe_page_pick(RecurWpePage *page, int index)
{
    if (!page->menu)
        return;
    if (index >= 0)
        webkit_option_menu_activate_item(page->menu, index);
    webkit_option_menu_close(page->menu);
    g_clear_object(&page->menu);
}

typedef struct {
    RecurWpeEvent cb;
    guintptr ctx;
} Reply;

static void on_evaluated(GObject *web, GAsyncResult *res, gpointer data)
{
    Reply *r = data;
    JSCValue *v = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(web), res, NULL);
    if (v && r->cb) {
        char *json = jsc_value_to_json(v, 0);
        if (json)
            r->cb(r->ctx, EV_REPLY, json, 0, 0, 0, NULL);
        g_free(json);
    }
    g_clear_object(&v);
    g_free(r);
}

/* Runs `js` in the page; with `reply` its JSON value comes back as EV_REPLY. */
void recur_wpe_page_eval(RecurWpePage *page, const char *js, gboolean reply)
{
    Reply *r = g_new0(Reply, 1);
    if (reply) {
        r->cb = page->cb;
        r->ctx = page->ctx;
    }
    webkit_web_view_evaluate_javascript(page->web, js, -1, NULL, NULL, NULL, on_evaluated, r);
}

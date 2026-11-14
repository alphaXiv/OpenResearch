#include <webkit2/webkit2.h>

static int result = 1;

static void received(WebKitUserContentManager *manager, WebKitJavascriptResult *message, gpointer data)
{
    (void)manager;
    (void)data;
    JSCValue *value = webkit_javascript_result_get_js_value(message);
    JSCValue *pass = jsc_value_is_object(value) ? jsc_value_object_get_property(value, "pass") : NULL;
    char *text = jsc_value_to_json(value, 0);
    g_print("%s\n", text ? text : "Invalid media probe response");
    result = text && pass && jsc_value_is_boolean(pass) && jsc_value_to_boolean(pass) ? 0 : 1;
    g_clear_object(&pass);
    g_free(text);
    gtk_main_quit();
}

static void terminated(WebKitWebView *view, WebKitWebProcessTerminationReason reason, gpointer data)
{
    (void)view;
    (void)data;
    g_printerr("WebKit web process terminated: %d\n", reason);
    result = 2;
    gtk_main_quit();
}

static gboolean timed_out(gpointer data)
{
    (void)data;
    g_printerr("WebKit media preview timed out\n");
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

int main(int argc, char **argv)
{
    if (argc != 2) return 1;
    char *html = NULL;
    GError *error = NULL;
    if (!g_file_get_contents(argv[1], &html, NULL, &error)) {
        g_printerr("%s\n", error->message);
        g_error_free(error);
        return 1;
    }
    gtk_init(&argc, &argv);
    GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    GtkWidget *view = webkit_web_view_new();
    WebKitUserContentManager *manager = webkit_web_view_get_user_content_manager(WEBKIT_WEB_VIEW(view));
    webkit_user_content_manager_register_script_message_handler(manager, "mediaProbe");
    g_signal_connect(manager, "script-message-received::mediaProbe", G_CALLBACK(received), NULL);
    g_signal_connect(view, "web-process-terminated", G_CALLBACK(terminated), NULL);
    gtk_container_add(GTK_CONTAINER(window), view);
    gtk_widget_show_all(window);
    webkit_web_view_load_html(WEBKIT_WEB_VIEW(view), html, "http://127.0.0.1/");
    g_free(html);
    g_timeout_add_seconds(25, timed_out, NULL);
    gtk_main();
    gtk_widget_destroy(window);
    return result;
}

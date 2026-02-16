#include <ibus.h>
#include <string.h>
#include <stdbool.h>
#include <stdio.h>
#include "wrapper.h"

static void* global_context = NULL;
static ibus_handy_callback_key_event global_key_event_cb = NULL;
static ibus_handy_callback_focus_in global_focus_in_cb = NULL;
static ibus_handy_callback_focus_out global_focus_out_cb = NULL;
static ibus_handy_callback_reset global_reset_cb = NULL;
static ibus_handy_callback_enable global_enable_cb = NULL;
static ibus_handy_callback_disable global_disable_cb = NULL;

#define IBUS_TYPE_HANDY_ENGINE (ibus_handy_engine_get_type())

GType ibus_handy_engine_get_type(void);

typedef struct {
    IBusEngineClass parent;
} IBusHandyEngineClass;

static void ibus_handy_engine_class_init(IBusHandyEngineClass *klass);
static void ibus_handy_engine_init(IBusHandyEngine *engine);
static void ibus_handy_engine_destroy(IBusHandyEngine *engine);

static gboolean ibus_handy_engine_process_key_event(
    IBusEngine *engine,
    guint keyval,
    guint keycode,
    guint modifiers
);

static void ibus_handy_engine_focus_in(IBusEngine *engine);
static void ibus_handy_engine_focus_out(IBusEngine *engine);
static void ibus_handy_engine_reset(IBusEngine *engine);
static void ibus_handy_engine_enable(IBusEngine *engine);
static void ibus_handy_engine_disable(IBusEngine *engine);

G_DEFINE_TYPE(IBusHandyEngine, ibus_handy_engine, IBUS_TYPE_ENGINE)

static void ibus_handy_engine_class_init(IBusHandyEngineClass *klass) {
    IBusObjectClass *ibus_object_class = IBUS_OBJECT_CLASS(klass);
    IBusEngineClass *engine_class = IBUS_ENGINE_CLASS(klass);

    ibus_object_class->destroy = (IBusObjectDestroyFunc)ibus_handy_engine_destroy;

    engine_class->process_key_event = ibus_handy_engine_process_key_event;
    engine_class->focus_in = ibus_handy_engine_focus_in;
    engine_class->focus_out = ibus_handy_engine_focus_out;
    engine_class->reset = ibus_handy_engine_reset;
    engine_class->enable = ibus_handy_engine_enable;
    engine_class->disable = ibus_handy_engine_disable;
}

static void ibus_handy_engine_init(IBusHandyEngine *engine) {
}

static void ibus_handy_engine_destroy(IBusHandyEngine *engine) {
    ((IBusObjectClass *)ibus_handy_engine_parent_class)
        ->destroy((IBusObject *)engine);
}

static gboolean ibus_handy_engine_process_key_event(
    IBusEngine *engine,
    guint keyval,
    guint keycode,
    guint modifiers
) {
    if (global_key_event_cb) {
        return global_key_event_cb(global_context, engine, keyval, keycode, modifiers);
    }
    return FALSE;
}

static void ibus_handy_engine_focus_in(IBusEngine *engine) {
    if (global_focus_in_cb) {
        global_focus_in_cb(global_context, engine);
    }
}

static void ibus_handy_engine_focus_out(IBusEngine *engine) {
    if (global_focus_out_cb) {
        global_focus_out_cb(global_context, engine);
    }
}

static void ibus_handy_engine_reset(IBusEngine *engine) {
    if (global_reset_cb) {
        global_reset_cb(global_context, engine);
    }
}

static void ibus_handy_engine_enable(IBusEngine *engine) {
    if (global_enable_cb) {
        global_enable_cb(global_context, engine);
    }
}

static void ibus_handy_engine_disable(IBusEngine *engine) {
    if (global_disable_cb) {
        global_disable_cb(global_context, engine);
    }
}

static void ibus_disconnected_cb(IBusBus *bus, gpointer user_data) {
    ibus_quit();
}

void ibus_handy_set_callback(
    void* ctx,
    ibus_handy_callback_key_event key_event_cb,
    ibus_handy_callback_focus_in focus_in_cb,
    ibus_handy_callback_focus_out focus_out_cb,
    ibus_handy_callback_reset reset_cb,
    ibus_handy_callback_enable enable_cb,
    ibus_handy_callback_disable disable_cb
) {
    global_context = ctx;
    global_key_event_cb = key_event_cb;
    global_focus_in_cb = focus_in_cb;
    global_focus_out_cb = focus_out_cb;
    global_reset_cb = reset_cb;
    global_enable_cb = enable_cb;
    global_disable_cb = disable_cb;
}

void ibus_handy_init(bool ibus_mode) {
    ibus_init();

    IBusBus *bus = ibus_bus_new();
    g_object_ref_sink(bus);
    g_signal_connect(bus, "disconnected", G_CALLBACK(ibus_disconnected_cb), NULL);

    IBusFactory *factory = ibus_factory_new(ibus_bus_get_connection(bus));
    g_object_ref_sink(factory);
    ibus_factory_add_engine(factory, "handy", IBUS_TYPE_HANDY_ENGINE);

    if (ibus_mode) {
        ibus_bus_request_name(bus, "org.freedesktop.IBus.Handy", 0);
    } else {
        IBusComponent *component;

        component = ibus_component_new(
            "org.freedesktop.IBus.Handy",
            "Handy Speech-to-Text",
            "0.7.5",
            "MIT",
            "Handy Team",
            "https://github.com/rohithmahesh/Handy",
            "",
            "handy-ibus"
        );

        ibus_component_add_engine(
            component,
            ibus_engine_desc_new(
                "handy",
                "Handy Speech-to-Text",
                "Voice dictation using Handy",
                "en",
                "MIT",
                "Handy Team",
                PKGDATADIR "/icons/handy.svg",
                "us"
            )
        );

        ibus_bus_register_component(bus, component);
    }
}

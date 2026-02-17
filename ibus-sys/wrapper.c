#include <ibus.h>
#include <string.h>
#include <stdbool.h>
#include <stdio.h>
#include "wrapper.h"

#ifndef HANDY_VERSION
#define HANDY_VERSION "unknown"
#endif

#define IBUS_BUS_NAME_REQUESTED_PRIMARY 1
#define IBUS_BUS_NAME_REQUESTED_REPLACED 2

static void* global_context = NULL;
static ibus_handy_callback_key_event global_key_event_cb = NULL;
static ibus_handy_callback_focus_in global_focus_in_cb = NULL;
static ibus_handy_callback_focus_out global_focus_out_cb = NULL;
static ibus_handy_callback_reset global_reset_cb = NULL;
static ibus_handy_callback_enable global_enable_cb = NULL;
static ibus_handy_callback_disable global_disable_cb = NULL;
static IBusBus *global_bus = NULL;
static IBusFactory *global_factory = NULL;

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
    (void)engine;
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
    (void)bus;
    (void)user_data;
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

int ibus_handy_init(bool ibus_mode) {
    ibus_init();

    IBusBus *bus = ibus_bus_new();
    if (!bus) {
        fprintf(stderr, "Failed to create IBus bus\n");
        return 1;
    }

    if (!ibus_bus_is_connected(bus)) {
        fprintf(stderr, "IBus daemon not running\n");
        g_object_unref(bus);
        return 2;
    }

    GDBusConnection *conn = ibus_bus_get_connection(bus);
    if (!conn) {
        fprintf(stderr, "IBus bus has no connection\n");
        g_object_unref(bus);
        return 3;
    }

    IBusFactory *factory = ibus_factory_new(conn);
    if (!factory) {
        fprintf(stderr, "Failed to create IBus factory\n");
        g_object_unref(bus);
        return 4;
    }

    g_signal_connect(bus, "disconnected", G_CALLBACK(ibus_disconnected_cb), NULL);

    ibus_factory_add_engine(factory, "handy", IBUS_TYPE_HANDY_ENGINE);

    global_bus = bus;
    global_factory = factory;

    if (ibus_mode) {
        guint result = ibus_bus_request_name(bus, "org.freedesktop.IBus.Handy", 0);
        if (result != IBUS_BUS_NAME_REQUESTED_PRIMARY && result != IBUS_BUS_NAME_REQUESTED_REPLACED) {
            fprintf(stderr, "Warning: Failed to acquire IBus name: %u\n", result);
        }
    } else {
        IBusComponent *component;

        component = ibus_component_new(
            "org.freedesktop.IBus.Handy",
            "Handy Speech-to-Text",
            HANDY_VERSION,
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
                "Handy",
                "Handy speech-to-text dictation",
                "other",
                "MIT",
                "Handy Team",
                PKGDATADIR "/icons/handy.svg",
                "default"
            )
        );

        ibus_bus_register_component(bus, component);
        g_object_unref(component);
    }

    return 0;
}

void ibus_handy_cleanup(void) {
    if (global_factory) {
        g_object_unref(global_factory);
        global_factory = NULL;
    }
    if (global_bus) {
        g_object_unref(global_bus);
        global_bus = NULL;
    }
}

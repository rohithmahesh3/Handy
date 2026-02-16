"""
IBus Engine for Handy Speech-to-Text.

This module provides the IBus engine implementation that allows Handy
to function as an input method in GNOME and other IBus-compatible
desktop environments.

Architecture:
    When the user switches to Handy as an input method (via Super+Space),
    recording starts automatically. When they switch away or focus changes,
    recording stops and the transcribed text is committed to the focused
    application.

Wayland-only:
    This engine is designed for Wayland compositors only. It relies on
    IBus to handle text input, which works natively on Wayland.
"""

import logging
from typing import Optional

import gi

gi.require_version("IBus", "1.0")
gi.require_version("GLib", "2.0")

from gi.repository import IBus
from gi.repository import GLib

from .dbus_client import HandyDBusClient

logger = logging.getLogger(__name__)

ENGINE_NAME = "handy"
ENGINE_LONG_NAME = "Handy Speech-to-Text"
ENGINE_DESCRIPTION = "Voice dictation using Handy"
ENGINE_LANGUAGE = "en"
ENGINE_LAYOUT = "us"


class HandyEngine(IBus.Engine):
    """
    IBus Engine for Handy Speech-to-Text.

    This engine starts recording when it receives focus (i.e., when the user
    switches to it via Super+Space) and stops recording when focus is lost.
    The transcribed text is committed directly to the application.
    """

    __gtype_name__ = "HandyEngine"

    def __init__(self):
        """Initialize the Handy IBus engine."""
        super().__init__()

        self._dbus_client = HandyDBusClient()
        self._dbus_client.set_callbacks(
            on_transcription_ready=self._on_transcription_ready,
            on_recording_changed=self._on_recording_changed,
            on_error=self._on_error,
        )

        self._is_recording = False
        self._is_focused = False
        self._reconnect_timer_id: Optional[int] = None

        # Try to connect
        self._try_connect()

        logger.info("HandyEngine initialized")

    def _try_connect(self) -> bool:
        """Attempt to connect to Handy D-Bus service."""
        if self._dbus_client.connect():
            logger.info("Connected to Handy")
            return True
        else:
            logger.warning("Failed to connect to Handy, will retry on focus")
            return False

    def do_focus_in(self):
        """Handle focus in event - start recording."""
        logger.debug("Focus in")
        self._is_focused = True

        if not self._dbus_client.is_connected:
            if not self._try_connect():
                return

        if not self._is_recording:
            self._start_recording()

    def do_focus_out(self):
        """Handle focus out event - stop recording and commit text."""
        logger.debug("Focus out")
        self._is_focused = False

        if self._is_recording:
            self._stop_and_commit()

    def do_reset(self):
        """Handle reset event - cancel current recording."""
        logger.debug("Reset")
        if self._is_recording:
            self._cancel_recording()

    def do_enable(self):
        """Handle enable event - engine is enabled."""
        logger.debug("Engine enabled")

    def do_disable(self):
        """Handle disable event - engine is disabled."""
        logger.debug("Engine disabled")
        self._is_focused = False
        if self._is_recording:
            self._cancel_recording()

    def do_process_key_event(self, keyval, keycode, state):
        """
        Handle key events.

        For speech-to-text, we generally don't process key events directly.
        However, we can use Escape to cancel recording.

        Args:
            keyval: The key value.
            keycode: The hardware keycode.
            state: The key state (pressed/released, modifiers).

        Returns:
            True if the event was handled, False to pass it on.
        """
        # Only handle key press events
        if state & IBus.ModifierType.RELEASE_MASK:
            return False

        # Escape cancels recording
        if keyval == IBus.Escape and self._is_recording:
            logger.debug("Escape pressed, cancelling recording")
            self._cancel_recording()
            return True

        # Pass all other keys through
        return False

    def _start_recording(self):
        """Start recording."""
        if self._is_recording:
            return

        logger.info("Starting recording")
        if self._dbus_client.start_recording():
            self._is_recording = True
        else:
            logger.error("Failed to start recording")
            self._show_error("Failed to start recording")

    def _stop_and_commit(self):
        """Stop recording and commit the transcription."""
        if not self._is_recording:
            return

        logger.info("Stopping recording")
        self._is_recording = False

        text = self._dbus_client.stop_recording()
        if text:
            self._commit_text(text)

    def _cancel_recording(self):
        """Cancel the current recording without committing."""
        if not self._is_recording:
            return

        logger.info("Cancelling recording")
        self._is_recording = False
        self._dbus_client.cancel_recording()

    def _commit_text(self, text: str):
        """
        Commit text to the focused application.

        Args:
            text: The text to commit.
        """
        if not text:
            return

        logger.info(f"Committing text: {text[:50]}...")

        # Create IBus text object and commit
        ibus_text = IBus.Text.new_from_string(text)
        self.commit_text(ibus_text)

    def _on_transcription_ready(self, text: str):
        """
        Callback for when transcription is ready.

        This is called when Handy emits the TranscriptionReady signal.
        If we're not focused anymore, we still commit the text.

        Args:
            text: The transcribed text.
        """
        logger.debug(f"Transcription ready: {text[:50]}...")

        # Always commit the text, even if we lost focus
        # (the recording was started while focused)
        self._commit_text(text)
        self._is_recording = False

    def _on_recording_changed(self, is_recording: bool):
        """
        Callback for recording state changes.

        Args:
            is_recording: Whether recording is active.
        """
        logger.debug(f"Recording state changed: {is_recording}")
        self._is_recording = is_recording

    def _on_error(self, message: str):
        """
        Callback for errors from Handy.

        Args:
            message: The error message.
        """
        logger.error(f"Handy error: {message}")
        self._show_error(message)
        self._is_recording = False

    def _show_error(self, message: str):
        """
        Show an error message to the user.

        Args:
            message: The error message.
        """
        # Update the preedit text to show an error briefly
        # This is a simple way to provide feedback
        ibus_text = IBus.Text.new_from_string(f"[Error: {message}]")
        self.update_preedit_text(ibus_text, 0, False)

        # Clear after a short delay
        GLib.timeout_add(2000, self._clear_preedit)

    def _clear_preedit(self) -> bool:
        """Clear the preedit text."""
        ibus_text = IBus.Text.new_from_string("")
        self.update_preedit_text(ibus_text, 0, False)
        return False  # Don't repeat the timer

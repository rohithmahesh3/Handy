"""
D-Bus client for communicating with Handy's transcription service.

This module provides a client that connects to the Handy D-Bus service
and allows the IBus engine to control recording and receive transcriptions.
"""

import logging
from typing import Optional, Tuple, Callable
from gi.repository import GLib
import dbus
import dbus.mainloop.glib

logger = logging.getLogger(__name__)

# D-Bus service details
HANDY_BUS_NAME = "com.handy.Transcription"
HANDY_OBJECT_PATH = "/com/handy/Transcription"
HANDY_INTERFACE = "com.handy.Transcription"


class HandyDBusClient:
    """Client for communicating with Handy via D-Bus."""

    def __init__(self):
        """Initialize the D-Bus client."""
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
        self._bus: Optional[dbus.SessionBus] = None
        self._proxy: Optional[dbus.Interface] = None
        self._connected = False
        self._on_transcription_ready: Optional[Callable[[str], None]] = None
        self._on_recording_changed: Optional[Callable[[bool], None]] = None
        self._on_error: Optional[Callable[[str], None]] = None

    def connect(self) -> bool:
        """
        Connect to the Handy D-Bus service.

        Returns:
            True if connected successfully, False otherwise.
        """
        try:
            self._bus = dbus.SessionBus()

            # Check if Handy is running
            if HANDY_BUS_NAME not in self._bus.list_names():
                logger.warning(
                    "Handy D-Bus service not found. Is Handy running with IBus mode enabled?"
                )
                return False

            # Get the proxy object
            proxy_obj = self._bus.get_object(HANDY_BUS_NAME, HANDY_OBJECT_PATH)
            self._proxy = dbus.Interface(proxy_obj, HANDY_INTERFACE)

            # Connect to signals
            self._proxy.connect_to_signal(
                "TranscriptionReady", self._on_transcription_ready_signal
            )
            self._proxy.connect_to_signal(
                "RecordingStateChanged", self._on_recording_changed_signal
            )
            self._proxy.connect_to_signal("Error", self._on_error_signal)

            self._connected = True
            logger.info("Connected to Handy D-Bus service")
            return True

        except dbus.DBusException as e:
            logger.error(f"Failed to connect to Handy D-Bus service: {e}")
            self._connected = False
            return False

    def disconnect(self):
        """Disconnect from the Handy D-Bus service."""
        self._proxy = None
        self._bus = None
        self._connected = False
        logger.info("Disconnected from Handy D-Bus service")

    @property
    def is_connected(self) -> bool:
        """Check if connected to the Handy service."""
        return self._connected and self._proxy is not None

    def set_callbacks(
        self,
        on_transcription_ready: Optional[Callable[[str], None]] = None,
        on_recording_changed: Optional[Callable[[bool], None]] = None,
        on_error: Optional[Callable[[str], None]] = None,
    ):
        """
        Set callback functions for D-Bus signals.

        Args:
            on_transcription_ready: Called when transcription is complete.
            on_recording_changed: Called when recording state changes.
            on_error: Called when an error occurs.
        """
        self._on_transcription_ready = on_transcription_ready
        self._on_recording_changed = on_recording_changed
        self._on_error = on_error

    def _on_transcription_ready_signal(self, text: str):
        """Handle TranscriptionReady signal."""
        logger.debug(f"Received transcription: {text[:50]}...")
        if self._on_transcription_ready:
            self._on_transcription_ready(text)

    def _on_recording_changed_signal(self, is_recording: bool):
        """Handle RecordingStateChanged signal."""
        logger.debug(f"Recording state changed: {is_recording}")
        if self._on_recording_changed:
            self._on_recording_changed(is_recording)

    def _on_error_signal(self, message: str):
        """Handle Error signal."""
        logger.error(f"Received error from Handy: {message}")
        if self._on_error:
            self._on_error(message)

    def start_recording(self) -> bool:
        """
        Start recording audio.

        Returns:
            True if recording started successfully.
        """
        if not self.is_connected:
            logger.warning("Cannot start recording: not connected to Handy")
            return False

        try:
            self._proxy.StartRecording()
            logger.info("Recording started")
            return True
        except dbus.DBusException as e:
            logger.error(f"Failed to start recording: {e}")
            return False

    def stop_recording(self) -> Optional[str]:
        """
        Stop recording and get the transcription.

        Returns:
            The transcribed text, or None if an error occurred.
        """
        if not self.is_connected:
            logger.warning("Cannot stop recording: not connected to Handy")
            return None

        try:
            text = str(self._proxy.StopRecording())
            logger.info(f"Recording stopped, transcription: {text[:50]}...")
            return text
        except dbus.DBusException as e:
            logger.error(f"Failed to stop recording: {e}")
            return None

    def cancel_recording(self) -> bool:
        """
        Cancel the current recording without transcription.

        Returns:
            True if cancelled successfully.
        """
        if not self.is_connected:
            return False

        try:
            self._proxy.CancelRecording()
            logger.info("Recording cancelled")
            return True
        except dbus.DBusException as e:
            logger.error(f"Failed to cancel recording: {e}")
            return False

    def get_state(self) -> Tuple[bool, bool]:
        """
        Get the current state.

        Returns:
            Tuple of (is_recording, is_model_loaded).
        """
        if not self.is_connected:
            return (False, False)

        try:
            is_recording, is_model_loaded = self._proxy.GetState()
            return (bool(is_recording), bool(is_model_loaded))
        except dbus.DBusException as e:
            logger.error(f"Failed to get state: {e}")
            return (False, False)

    def get_language(self) -> Optional[str]:
        """
        Get the currently selected language.

        Returns:
            The language code (e.g., "en", "zh-Hans") or None.
        """
        if not self.is_connected:
            return None

        try:
            return str(self._proxy.GetLanguage())
        except dbus.DBusException as e:
            logger.error(f"Failed to get language: {e}")
            return None

    def set_language(self, language: str) -> bool:
        """
        Set the language for transcription.

        Args:
            language: The language code (e.g., "en", "zh-Hans").

        Returns:
            True if set successfully.
        """
        if not self.is_connected:
            return False

        try:
            self._proxy.SetLanguage(language)
            logger.info(f"Language set to: {language}")
            return True
        except dbus.DBusException as e:
            logger.error(f"Failed to set language: {e}")
            return False

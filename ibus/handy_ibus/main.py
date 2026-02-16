"""
Main entry point for the Handy IBus engine.

This script initializes the IBus engine and starts the main loop.
It is called by IBus when the user selects Handy as an input method.
"""

import argparse
import logging
import sys

import gi

gi.require_version("GLib", "2.0")
gi.require_version("IBus", "1.0")

from gi.repository import GLib
from gi.repository import IBus

from .engine import HandyEngine, ENGINE_NAME, ENGINE_LONG_NAME, ENGINE_DESCRIPTION

logger = logging.getLogger(__name__)


def setup_logging(verbose: bool = False):
    """Configure logging."""
    level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(
        level=level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
    )


def main():
    """Main entry point for the IBus engine."""
    parser = argparse.ArgumentParser(description="Handy IBus Engine")
    parser.add_argument(
        "--ibus",
        action="store_true",
        help="Run as IBus engine (started by IBus daemon)",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Enable verbose logging",
    )
    parser.add_argument(
        "--version",
        action="version",
        version="%(prog)s 0.1.0",
    )
    args = parser.parse_args()

    setup_logging(args.verbose)
    logger.info("Starting Handy IBus Engine")

    if not args.ibus:
        logger.warning("Not started with --ibus flag. This is for testing only.")

    # Initialize IBus
    bus = IBus.Bus()
    if not bus.is_connected():
        logger.error("Failed to connect to IBus daemon")
        sys.exit(1)

    logger.info("Connected to IBus daemon")

    # Create the factory
    factory = IBus.Factory(
        path=IBus.path_to_factory("com.handy.IBus.Factory"),
        connection=bus.get_connection(),
    )

    # Register the engine
    factory.add_engine(
        ENGINE_NAME,
        HandyEngine,
    )

    # Register the factory with IBus
    bus.request_name("com.handy.IBus", 0)

    # If started with --ibus, we need to register with IBus properly
    if args.ibus:
        bus.register_component(
            IBus.Component(
                name="com.handy.IBus",
                description="Handy Speech-to-Text IBus Engine",
                version="0.1.0",
                license="MIT",
                author="Handy Team",
                homepage="https://github.com/rohithmahesh/Handy",
                textdomain="handy-ibus",
            )
        )

    logger.info("Handy IBus Engine registered")

    # Run the main loop
    loop = GLib.MainLoop()
    try:
        loop.run()
    except KeyboardInterrupt:
        logger.info("Shutting down")
        loop.quit()

    return 0


if __name__ == "__main__":
    sys.exit(main())

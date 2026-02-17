# User Guide

This is the guide for how to use RFR (Rust Flight Recording) to instrument your own application and
everything you can do with the recordings from that application once you have them.

## Recording

The [Recording section] explains how to instrument your application to create flight recordings.

## Visualizing

The [Visualizing section] explains how to visualize a recording. It is intended for post-mortum use,
but it is possible to copy out a partial recording to visualize while the application continues to
run.

## Converting

The [Converting section] explains how to convert a recording to formats that other tools can
consume. The only supported format is Perfetto Trace Format.

[Recording section]: user-guide/recording.md
[Visualizing section]: user-guide/visualizing.md
[Converting section]: user-guide/converting.md

{ pkgs, inputs, ... }:
{
  # stainless-facts is a single crate, not a polylith workspace.
  metadev.polylith.enable = false;

  languages.rust = {
    enable = true;
    channel = "stable";
  };

  packages = [ pkgs.cargo-watch ];

  env.METADEV_PROJECT = "stainless-facts";
}

{
  description = "reactive-vcr development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib stdenv;
          # Runtime and headers for decoding, preview and rendering.
          gst = with pkgs.gst_all_1; [
            gstreamer
            gst-plugins-base
            gst-plugins-good
            gst-plugins-bad
            gst-plugins-ugly
            gst-libav
          ];
          # Libraries eframe/winit load at runtime on Linux.
          linuxRuntime = with pkgs; [
            libxkbcommon
            wayland
            libGL
            libx11
            libxcursor
            libxi
            libxrandr
          ];
        in
        {
          default = pkgs.mkShell (
            {
              packages =
                (with pkgs; [
                  cargo
                  rustc
                  rustfmt
                  clippy
                  rust-analyzer
                  # demo generator and `just demo` inputs
                  ffmpeg
                  python3
                  just
                  pkg-config
                ])
                ++ gst
                ++ lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.libiconv ]
                ++ lib.optionals stdenv.hostPlatform.isLinux linuxRuntime;
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
              shellHook = ''
                export GST_PLUGIN_SYSTEM_PATH_1_0=${lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gst}
              '';
            }
            // lib.optionalAttrs stdenv.hostPlatform.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath linuxRuntime;
            }
          );
        }
      );
    };
}

{ config, lib, pkgs, ... }:
let
  cfg = config.programs.fterm;
in
{
  options.programs.fterm = {
    enable = lib.mkEnableOption "fterm, a serial terminal for Forth targets";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "fterm built from its flake";
      description = "The fterm package.";
    };
    init = lib.mkOption {
      type = lib.types.lines;
      default = "";
      example = ''
        3000 timeout
        2 timestamps
        : reboot  s" reset" run  2000 delay ;
        ' reboot bind F6
      '';
      description = ''
        Host Forth written to ~/.config/fterm/init.fs. Used when no project
        .fterm.fs exists; a project config loads it with `include-global`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile."fterm/init.fs" = lib.mkIf (cfg.init != "") { text = cfg.init; };
  };
}

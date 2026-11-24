#!/bin/bash
# Calibrates a touch screen on the framebuffer (no X, no Wayland) for libinput.
#
# tslib's ts_calibrate shows five crosshairs on the framebuffer and fits raw touch values to screen pixels. This
# script turns that fit into libinput's calibration matrix (normalized device coordinates, see the libinput docs
# "Static device configuration via udev", LIBINPUT_CALIBRATION_MATRIX) and stores it as a udev rule for this
# device. Every program that reads the touch through libinput then gets calibrated positions.
#
# A resistive panel (e.g. a TFT35 with an ADS7846 or TSC2007) that is not calibrated reports positions off target,
# often pulled towards the middle; narrow buttons at the edges then miss.
#
#   sudo bash touch-calibrate.sh [-f /dev/fbN] [-s service-to-stop]
#
# The running Sailhand service (sailhand from the zip, sailhand@<user> from a package; -s names another) is stopped
# while the crosshairs show, since both draw on the framebuffer, and started again afterwards. Undo: delete
# /etc/udev/rules.d/99-touch-calibration.rules, then `udevadm control --reload && udevadm trigger` and restart the
# service. The rule belongs to the display, so uninstalling Sailhand leaves it in place.
set -euo pipefail

framebuffer=/dev/fb0
services=()
while getopts "f:s:" option; do
  case $option in
    f) framebuffer=$OPTARG ;;
    s) services=("$OPTARG") ;;
    *) echo "usage: $0 [-f /dev/fbN] [-s service]"; exit 2 ;;
  esac
done
rule=/etc/udev/rules.d/99-touch-calibration.rules

if [[ $EUID -ne 0 ]]; then
  echo "Run it with sudo." >&2
  exit 1
fi

# The touch screen as udev sees it.
touch_device=""
for event in /dev/input/event*; do
  if udevadm info -q property -n "$event" | grep -q '^ID_INPUT_TOUCHSCREEN=1$'; then
    touch_device=$event
    break
  fi
done
if [[ -z $touch_device ]]; then
  echo "No touch screen found (no input device with ID_INPUT_TOUCHSCREEN=1)." >&2
  exit 1
fi
device_name=$(cat "/sys/class/input/$(basename "$touch_device")/device/name")
read -r width height < <(tr ',' ' ' < "/sys/class/graphics/$(basename "$framebuffer")/virtual_size")
echo "Touch screen: $device_name ($touch_device), framebuffer $framebuffer ${width}x${height}"

if ! command -v ts_calibrate > /dev/null; then
  echo "Installing tslib's tools (libts-bin) ..."
  apt-get install -y libts-bin
fi

if [[ ${#services[@]} -eq 0 ]]; then
  mapfile -t services < <(systemctl list-units --type=service --state=running --no-legend --plain \
    'sailhand.service' 'sailhand@*.service' | awk '{print $1}')
fi

work=$(mktemp -d)
restore() {
  rm -rf "$work"
  for service in "${services[@]}"; do
    systemctl start "$service" || true
  done
}
trap restore EXIT
cat > "$work/ts.conf" << 'EOF'
module_raw input
module pthres pmin=1
module dejitter delta=100
module linear
EOF

for service in "${services[@]}"; do
  echo "Stopping $service while the crosshairs show ..."
  systemctl stop "$service"
done
echo "Tap the centre of each crosshair on the screen (a fingernail or stylus is more precise) ..."
TSLIB_FBDEVICE=$framebuffer TSLIB_TSDEVICE=$touch_device TSLIB_CONFFILE=$work/ts.conf \
  TSLIB_CALIBFILE=$work/pointercal ts_calibrate

# tslib's linear fit (pointercal, first seven numbers p0..p6): x_screen = (p0*x + p1*y + p2) / p6,
# y_screen = (p3*x + p4*y + p5) / p6, with x and y in raw device units. libinput applies its matrix to coordinates
# normalized by the device's range (ABS_MT_POSITION_* when present, else ABS_X/ABS_Y), so the raw range is needed.
matrix=$(python3 -I - "$touch_device" "$work/pointercal" "$width" "$height" << 'EOF'
import fcntl, struct, sys

device, pointercal, width, height = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])

def absinfo(handle, axis):
    # EVIOCGABS(axis) = _IOR('E', 0x40 + axis, struct input_absinfo), six int32.
    request = (2 << 30) | (24 << 16) | (ord("E") << 8) | (0x40 + axis)
    _value, minimum, maximum, *_ = struct.unpack("6i", fcntl.ioctl(handle, request, bytes(24)))
    return minimum, maximum

with open(device, "rb") as handle:
    x_range, y_range = absinfo(handle, 0x35), absinfo(handle, 0x36)  # ABS_MT_POSITION_X/Y
    if x_range[1] <= x_range[0]:
        x_range, y_range = absinfo(handle, 0x00), absinfo(handle, 0x01)  # ABS_X/Y

p = [float(value) for value in open(pointercal).read().split()[:7]]
(x_min, x_max), (y_min, y_max) = x_range, y_range
x_span, y_span = x_max - x_min, y_max - y_min
a, b = p[0] * x_span / (p[6] * width), p[1] * y_span / (p[6] * width)
c = (p[0] * x_min + p[1] * y_min + p[2]) / (p[6] * width)
d, e = p[3] * x_span / (p[6] * height), p[4] * y_span / (p[6] * height)
f = (p[3] * x_min + p[4] * y_min + p[5]) / (p[6] * height)
print(f"raw range x {x_min}..{x_max}, y {y_min}..{y_max}", file=sys.stderr)
print(" ".join(f"{value:.6f}" for value in (a, b, c, d, e, f)))
EOF
)
echo "Calibration matrix: $matrix"

read -r -p "Write it to $rule and reload udev? [y/N] " answer
if [[ $answer != [yY] ]]; then
  echo "Nothing written."
  exit 0
fi
printf 'ACTION=="add|change", KERNEL=="event*", ATTRS{name}=="%s", ENV{LIBINPUT_CALIBRATION_MATRIX}="%s"\n' \
  "$device_name" "$matrix" > "$rule"
udevadm control --reload
udevadm trigger --action=change "$touch_device"
# The trigger works in the background; the services started on exit must find the matrix already.
udevadm settle
echo "Written: $rule"
udevadm info -q property -n "$touch_device" | grep LIBINPUT_CALIBRATION_MATRIX || \
  echo "Warning: udev does not show the matrix on $touch_device yet." >&2

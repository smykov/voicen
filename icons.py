from PIL import Image, ImageDraw

from state import AppState


_SIZE = 32
_C = _SIZE // 2
_R = 12

_COLORS = {
    AppState.IDLE: (140, 140, 140),
    AppState.RECORDING: (220, 40, 40),
    AppState.DONE: (50, 180, 50),
    AppState.ERROR: (200, 30, 30),
}


def _circle(draw: ImageDraw.ImageDraw, fill: tuple[int, int, int]) -> None:
    draw.ellipse([_C - _R, _C - _R, _C + _R, _C + _R], fill=fill)


def _render_idle(draw: ImageDraw.ImageDraw) -> None:
    _circle(draw, _COLORS[AppState.IDLE])


def _render_recording(draw: ImageDraw.ImageDraw) -> None:
    _circle(draw, _COLORS[AppState.RECORDING])
    draw.ellipse([_C - 5, _C - 5, _C + 5, _C + 5], fill=(255, 255, 255))


def _render_transcribing(draw: ImageDraw.ImageDraw) -> None:
    draw.ellipse([_C - _R, _C - _R, _C + _R, _C + _R], fill=(255, 230, 50))
    draw.arc([_C - 5, _C - 5, _C + 5, _C + 5], start=0, end=300, fill=(200, 140, 0), width=3)


def _render_done(draw: ImageDraw.ImageDraw) -> None:
    _circle(draw, _COLORS[AppState.DONE])
    draw.line([(_C - 5, _C), (_C - 1, _C + 4), (_C + 6, _C - 4)], fill=(255, 255, 255), width=3)


def _render_error(draw: ImageDraw.ImageDraw) -> None:
    _circle(draw, _COLORS[AppState.ERROR])
    draw.line([(_C - 4, _C - 4), (_C + 4, _C + 4)], fill=(255, 255, 255), width=3)
    draw.line([(_C + 4, _C - 4), (_C - 4, _C + 4)], fill=(255, 255, 255), width=3)


_RENDERERS = {
    AppState.IDLE: _render_idle,
    AppState.RECORDING: _render_recording,
    AppState.TRANSCRIBING: _render_transcribing,
    AppState.DONE: _render_done,
    AppState.ERROR: _render_error,
}


def get_icon(state: AppState) -> Image.Image:
    img = Image.new("RGBA", (_SIZE, _SIZE), (0, 0, 0, 0))
    renderer = _RENDERERS[state]
    renderer(ImageDraw.Draw(img))
    return img.resize((24, 24), Image.LANCZOS)

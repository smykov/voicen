import glob
import os
import threading
import tkinter as tk
from tkinter import ttk, messagebox
from typing import Optional

import autostart
from config import AppConfig, save_config


class SettingsWindow:
    _INSTANCE: Optional["SettingsWindow"] = None

    _FIELDS = [
        ("API URL", "api_url", False, False, None),
        ("API Key", "api_key", False, False, None),
        ("Language", "language", False, True, ["ru", "en", "auto"]),
        ("Model", "model", False, True, ["large-v3", "turbo"]),
        ("Sample Rate", "sample_rate", True, False, None),
        ("Channels", "channels", True, False, None),
        ("Timeout (s)", "transcription_timeout", True, False, None),
    ]

    def __init__(self, config: AppConfig, history_dir: str):
        self._config = config
        self._history_dir = history_dir
        self._root: Optional[tk.Tk] = None
        self._entries: dict[str, tk.StringVar] = {}

    @classmethod
    def open_or_focus(cls, config: AppConfig, history_dir: str) -> None:
        if cls._INSTANCE is not None and cls._INSTANCE._root is not None:
            try:
                cls._INSTANCE._root.lift()
                cls._INSTANCE._root.focus_force()
            except tk.TclError:
                pass
            return
        win = cls(config, history_dir)
        cls._INSTANCE = win
        win.show()

    def show(self) -> None:
        t = threading.Thread(target=self._run, daemon=True)
        t.start()

    def _run(self) -> None:
        self._root = tk.Tk()
        self._root.title("Транскрибатор — Settings")
        self._root.resizable(False, False)
        self._root.protocol("WM_DELETE_WINDOW", self._close)

        nb = ttk.Notebook(self._root)
        nb.pack(fill=tk.BOTH, expand=True, padx=8, pady=8)

        general = ttk.Frame(nb, padding=12)
        nb.add(general, text="General")

        history = ttk.Frame(nb, padding=12)
        nb.add(history, text="History")

        self._build_general(general)
        self._build_history(history)

        self._root.mainloop()

    # -- General tab --

    def _build_general(self, parent: ttk.Frame) -> None:
        for row, (label, attr, is_int, is_combo, choices) in enumerate(self._FIELDS):
            tk.Label(parent, text=label, anchor=tk.W, width=14).grid(
                row=row, column=0, sticky=tk.W, pady=3
            )
            var = tk.StringVar(value=str(getattr(self._config, attr, "")))
            self._entries[label] = var
            if is_combo:
                w = ttk.Combobox(parent, textvariable=var, values=choices, state="readonly")
            else:
                w = ttk.Entry(parent, textvariable=var)
            w.grid(row=row, column=1, sticky=tk.EW, pady=3, padx=(0, 8))

        parent.columnconfigure(1, weight=1)

        row = len(self._FIELDS)

        self._auto_paste_var = tk.BooleanVar(value=self._config.auto_paste)
        ttk.Checkbutton(parent, text="Auto-paste after transcription", variable=self._auto_paste_var).grid(
            row=row, column=0, columnspan=2, sticky=tk.W, pady=(8, 0)
        )
        row += 1

        self._save_history_var = tk.BooleanVar(value=self._config.save_history)
        ttk.Checkbutton(parent, text="Save history to daily files", variable=self._save_history_var).grid(
            row=row, column=0, columnspan=2, sticky=tk.W, pady=(2, 0)
        )
        row += 1

        self._auto_start_var = tk.BooleanVar(value=self._config.auto_start)
        ttk.Checkbutton(parent, text="Run at Windows startup", variable=self._auto_start_var).grid(
            row=row, column=0, columnspan=2, sticky=tk.W, pady=(2, 8)
        )
        row += 1

        btn_frame = ttk.Frame(parent)
        btn_frame.grid(row=row, column=0, columnspan=2, pady=(8, 0))
        ttk.Button(btn_frame, text="Save", command=self._save).pack(side=tk.LEFT, padx=(0, 8))
        ttk.Button(btn_frame, text="Cancel", command=self._close).pack(side=tk.LEFT)

    def _save(self) -> None:
        for label, attr, is_int, _is_combo, _choices in self._FIELDS:
            raw = self._entries[label].get().strip()
            if not raw:
                messagebox.showerror("Validation error", f'"{label}" cannot be empty.', parent=self._root)
                return
            try:
                value = int(raw) if is_int else raw
            except ValueError:
                messagebox.showerror("Validation error", f'"{label}" must be a number.', parent=self._root)
                return
            setattr(self._config, attr, value)

        self._config.auto_paste = self._auto_paste_var.get()
        self._config.save_history = self._save_history_var.get()
        self._config.auto_start = self._auto_start_var.get()

        save_config(self._config)

        if self._config.auto_start:
            autostart.enable()
        else:
            autostart.disable()
        messagebox.showinfo("Saved", "Settings saved.\nRestart the app for changes to take effect.", parent=self._root)
        self._close()

    def _close(self) -> None:
        SettingsWindow._INSTANCE = None
        if self._root:
            self._root.destroy()

    # -- History tab --

    def _build_history(self, parent: ttk.Frame) -> None:
        paned = ttk.PanedWindow(parent, orient=tk.HORIZONTAL)
        paned.pack(fill=tk.BOTH, expand=True)

        left = ttk.Frame(paned)
        paned.add(left, weight=1)

        tk.Label(left, text="Files", anchor=tk.W, font=("", 10, "bold")).pack(fill=tk.X)

        self._files_listbox = tk.Listbox(left, width=28)
        self._files_listbox.pack(fill=tk.BOTH, expand=True, pady=(4, 0))
        self._files_listbox.bind("<<ListboxSelect>>", self._on_file_select)

        right = ttk.Frame(paned)
        paned.add(right, weight=2)

        tk.Label(right, text="Contents", anchor=tk.W, font=("", 10, "bold")).pack(fill=tk.X)

        self._contents_text = tk.Text(right, wrap=tk.WORD, state=tk.DISABLED)
        self._contents_text.pack(fill=tk.BOTH, expand=True, pady=(4, 0))

        self._refresh_history()

    def _refresh_history(self) -> None:
        self._files_listbox.delete(0, tk.END)
        pattern = os.path.join(self._history_dir, "history-*.txt")
        files = sorted(glob.glob(pattern), reverse=True)
        self._history_files = files
        for f in files:
            self._files_listbox.insert(tk.END, os.path.basename(f))

    def _on_file_select(self, event: object) -> None:
        sel = self._files_listbox.curselection()
        if not sel or not self._history_files:
            return
        idx = sel[0]
        path = self._history_files[idx]
        try:
            with open(path, encoding="utf-8") as f:
                content = f.read()
        except OSError as e:
            content = f"Error reading file: {e}"

        self._contents_text.config(state=tk.NORMAL)
        self._contents_text.delete("1.0", tk.END)
        self._contents_text.insert("1.0", content)
        self._contents_text.config(state=tk.DISABLED)

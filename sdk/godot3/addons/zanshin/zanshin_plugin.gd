tool
extends EditorPlugin

const AUTOLOAD = "Zanshin"

func _enter_tree() -> void:
	add_autoload_singleton(AUTOLOAD, "res://addons/zanshin/zanshin.gd")

func _exit_tree() -> void:
	remove_autoload_singleton(AUTOLOAD)

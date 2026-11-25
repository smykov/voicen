# stale-db fixture lazy: NOTICES ?= is refused: an environment variable can redirect it (T-181)
NOTICES ?= THIRD-PARTY-NOTICES.txt
# Padding: 10^4 variables generated here, so the committed file stays small while make -p
# prints ~1.2 MB (in hash order: the NOTICES line is NOT last, bytes of the dump follow it).
W := a b c d e f g h i j
$(foreach a,$(W),$(foreach b,$(W),$(foreach c,$(W),$(foreach d,$(W),$(eval PAD_$(a)$(b)$(c)$(d) := padding-value-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx)))))

check: licenses-check

licenses-check: licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale

licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale:
	@true

.PHONY: check licenses-check licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale

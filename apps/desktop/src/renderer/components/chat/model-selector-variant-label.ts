const DEFAULT_VARIANT_LABEL = "default"

function capitalize(value: string): string {
	if (!value) return value
	return `${value.charAt(0).toUpperCase()}${value.slice(1)}`
}

export function getVariantTriggerLabel(variant: string | undefined): string {
	if (!variant) return DEFAULT_VARIANT_LABEL
	if (variant === "disabled" || variant === "off") return "off"
	if (variant === "enabled") return "on"
	return variant.toLowerCase()
}

export function getVariantMenuLabel(variant: string | undefined): string {
	if (!variant) return capitalize(DEFAULT_VARIANT_LABEL)
	if (variant === "disabled" || variant === "off") return "Off"
	if (variant === "enabled" || variant === "on") return "On"
	return capitalize(variant)
}

export function resolveSelectedVariant(
	variants: string[],
	selectedVariant: string | undefined,
	currentVariant: string | undefined,
	allowDefaultVariant: boolean,
): string | undefined {
	if (selectedVariant && variants.includes(selectedVariant)) return selectedVariant
	if (currentVariant && variants.includes(currentVariant)) return currentVariant
	return allowDefaultVariant ? undefined : variants[0]
}

export interface ModelSelectorVariantOption {
	key: string
	value: string | undefined
	label: string
}

export function getVariantOptions(
	variants: string[],
	allowDefaultVariant: boolean,
): ModelSelectorVariantOption[] {
	const options = variants.map((variant) => ({
		key: variant,
		value: variant,
		label: getVariantMenuLabel(variant),
	}))
	if (!allowDefaultVariant) return options
	return [{ key: "__default__", value: undefined, label: getVariantMenuLabel(undefined) }, ...options]
}

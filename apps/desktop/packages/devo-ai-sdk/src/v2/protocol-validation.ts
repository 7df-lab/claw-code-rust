import Ajv, { type ErrorObject, type ValidateFunction } from "ajv"
import schemaBundle from "./generated/schema.json"

export type ProtocolValidationDirection =
	| "outgoingRequest"
	| "incomingResult"
	| "incomingNotification"
	| "incomingRequest"
	| "outgoingResponse"

type MethodSchemaBinding = Partial<Record<ProtocolValidationDirection, string>>

type ProtocolSchemaBundle = {
	schemas: Record<string, unknown>
	methods: Record<string, MethodSchemaBinding>
}

export type ProtocolValidationInput = {
	method: string
	direction: ProtocolValidationDirection
	payload: unknown
}

export class ProtocolValidationError extends Error {
	readonly method: string
	readonly direction: ProtocolValidationDirection
	readonly schemaName?: string
	readonly errors: ErrorObject[]
	readonly payload: unknown

	constructor(params: {
		method: string
		direction: ProtocolValidationDirection
		schemaName?: string
		errors?: ErrorObject[]
		payload: unknown
		message?: string
	}) {
		const schemaSuffix = params.schemaName ? ` against ${params.schemaName}` : ""
		const errorText = params.errors?.length ? `: ${formatAjvErrors(params.errors)}` : ""
		super(
			params.message ??
				`invalid ${params.direction} payload for ${params.method}${schemaSuffix}${errorText}`,
		)
		this.name = "ProtocolValidationError"
		this.method = params.method
		this.direction = params.direction
		this.schemaName = params.schemaName
		this.errors = params.errors ?? []
		this.payload = params.payload
	}
}

const bundle = schemaBundle as ProtocolSchemaBundle
const ajv = new Ajv({ allErrors: true, strict: false, validateFormats: false })
const validators = new Map<string, ValidateFunction>()

export function assertValidProtocolPayload<T = unknown>({
	method,
	direction,
	payload,
}: ProtocolValidationInput): T {
	const binding = bindingForMethod(method)
	if (!binding) {
		throw new ProtocolValidationError({
			method,
			direction,
			payload,
			message: `unknown protocol method ${method}`,
		})
	}

	const schemaName = binding[direction]
	if (!schemaName) return payload as T

	const validate = validatorForSchema(method, direction, schemaName, payload)
	if (validate(payload)) return payload as T

	throw new ProtocolValidationError({
		method,
		direction,
		schemaName,
		errors: validate.errors ?? [],
		payload,
	})
}

function bindingForMethod(method: string): MethodSchemaBinding | undefined {
	return bundle.methods[method]
}

function validatorForSchema(
	method: string,
	direction: ProtocolValidationDirection,
	schemaName: string,
	payload: unknown,
): ValidateFunction {
	const existing = validators.get(schemaName)
	if (existing) return existing

	const schema = bundle.schemas[schemaName]
	if (!schema) {
		throw new ProtocolValidationError({
			method,
			direction,
			schemaName,
			payload,
			message: `missing generated protocol schema ${schemaName} for ${method}`,
		})
	}

	const validate = ajv.compile(schema)
	validators.set(schemaName, validate)
	return validate
}

function formatAjvErrors(errors: ErrorObject[]): string {
	return errors
		.slice(0, 3)
		.map((error) => `${error.instancePath || "/"} ${error.message ?? "is invalid"}`)
		.join("; ")
}

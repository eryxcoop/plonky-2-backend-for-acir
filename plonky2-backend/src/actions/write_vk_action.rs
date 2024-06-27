use super::*;

pub struct WriteVKAction {
    pub bytecode_path: String,
    pub vk_path_output: String
}

impl WriteVKAction {
    pub fn run(&self) {
        let acir_program: Program = deserialize_program_within_file_path(&self.bytecode_path);
        let acir_circuit = &acir_program.functions[0];
        let mut translator = CircuitBuilderFromAcirToPlonky2::new();
        translator.translate_circuit(acir_circuit);
        let CircuitBuilderFromAcirToPlonky2 { builder, witness_target_map: _ } = translator;
        let plonky2_circuit = builder.build::<C>();
        let verifier_data = plonky2_circuit.verifier_data();
        let gate_serializer = DefaultGateSerializer;
        let serialized_verifier_data = verifier_data.to_bytes(&gate_serializer).unwrap();
        write_bytes_to_file_path(serialized_verifier_data, &self.vk_path_output);
    }
}
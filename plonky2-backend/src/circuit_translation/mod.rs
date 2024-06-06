mod tests;
pub mod assert_zero_translator;

use std::collections::{HashMap};
use std::error::Error;
use acir::circuit::{Circuit};
use acir::circuit::{ExpressionWidth, PublicInputs};
use acir::FieldElement;
use acir::native_types::{Expression, Witness};
use plonky2::field::goldilocks_field::GoldilocksField;
use plonky2::field::types::{Field, Field64};
use plonky2::iop::target::{BoolTarget, Target};
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2::plonk::circuit_data::CircuitConfig;
use plonky2::plonk::circuit_data::CircuitData;
use plonky2::plonk::config::{GenericConfig, KeccakGoldilocksConfig};
use num_bigint::BigUint;
use plonky2::iop::witness::PartialWitness;
use plonky2::iop::witness::WitnessWrite;
use plonky2::plonk::proof::ProofWithPublicInputs;
use std::collections::BTreeSet;
use acir::circuit::Opcode;
use acir::circuit::opcodes;
use acir::circuit::opcodes::{FunctionInput, MemOp};

const D: usize = 2;

type C = KeccakGoldilocksConfig;
type F = <C as GenericConfig<D>>::F;
type CB = CircuitBuilder::<F, D>;

#[derive(Clone, Debug)]
pub struct ByteTarget {
    pub bits: Vec<BoolTarget>,
}

pub struct CircuitBuilderFromAcirToPlonky2 {
    pub builder: CB,
    pub witness_target_map: HashMap<Witness, Target>,
}

impl CircuitBuilderFromAcirToPlonky2 {
    pub fn new() -> Self {
        let config = CircuitConfig::standard_recursion_config();
        let mut builder = CB::new(config);
        let mut witness_target_map: HashMap<Witness, Target> = HashMap::new();
        Self { builder, witness_target_map }
    }

    pub fn unpack(self) -> (CircuitData<F, C, 2>, HashMap<Witness, Target>){
        (self.builder.build::<C>(), self.witness_target_map)
    }

    pub fn translate_circuit(self: &mut Self, circuit: &Circuit) {
        self._register_public_parameters_from_acir_circuit(circuit);
        for opcode in &circuit.opcodes {
            match opcode {
                Opcode::AssertZero(expr) => {
                    eprintln!("----------ASSERT ZERO--------");
                    eprintln!("EXPR: {:?}", expr);
                    self._register_intermediate_witnesses_for_assert_zero(&expr);
                    self._translate_assert_zero(&expr);
                }
                Opcode::BrilligCall { id, inputs, outputs, predicate } => {
                    eprintln!("----------Brillig--------");
                    eprintln!("id: {:?}", id);
                    eprintln!("inputs: {:?}", inputs);
                    eprintln!("outputs: {:?}", outputs);
                    eprintln!("predicate: {:?}", predicate);
                }
                Opcode::MemoryInit { block_id, init } => {
                    eprintln!("outputs: {:?}", block_id);
                    eprintln!("predicate: {:?}", init);
                }
                Opcode::MemoryOp { block_id, op, predicate } => {
                    // TODO: check whether we should register if the predicate is false
                    self._register_intermediate_witnesses_for_memory_op(&op);
                }
                Opcode::BlackBoxFuncCall(func_call) => {
                    eprintln!("{:?}", func_call);
                    match func_call {
                        opcodes::BlackBoxFuncCall::RANGE { input } => {
                            eprintln!("{:?}", input);
                            let long_max_bits = input.num_bits.clone() as usize;
                            assert!(long_max_bits <= 32,
                                    "Range checks with more than 32 bits are not allowed yet while using Plonky2 prover");
                            let witness = input.witness;
                            let target = self._get_or_create_target_for_witness(witness);
                            self.builder.range_check(target, long_max_bits)
                        }
                        opcodes::BlackBoxFuncCall::AND { lhs, rhs, output } => {
                            self._extend_circuit_with_bitwise_u8_operation(lhs, rhs, output, Self::and);
                        }
                        opcodes::BlackBoxFuncCall::XOR { lhs, rhs, output } => {
                            self._extend_circuit_with_bitwise_u8_operation(lhs, rhs, output, Self::xor);
                        }
                        blackbox_func => {
                            panic!("Blackbox func not supported yet: {:?}", blackbox_func);
                        }
                    };
                }

                opcode => {
                    panic!("Opcode not supported yet: {:?}", opcode);
                }
            }
        }
    }

    fn _extend_circuit_with_bitwise_u8_operation(self: &mut Self, lhs: &FunctionInput, rhs: &FunctionInput,
                                                 output: &Witness, operation: fn(&mut Self, BoolTarget, BoolTarget) -> BoolTarget) {
        let lhs_byte_target = self._byte_target_for_witness(lhs.witness);
        let rhs_byte_target = self._byte_target_for_witness(rhs.witness);

        let output_byte_target = self._translate_u8_bitwise_operation(
            lhs_byte_target, rhs_byte_target, operation);

        let output_target = self.convert_byte_to_u8(output_byte_target);
        self.witness_target_map.insert(*output, output_target);
    }

    fn _byte_target_for_witness(self: &mut Self, w: Witness) -> ByteTarget {
        let target = self._get_or_create_target_for_witness(w);
        self.convert_u8_to_byte(target)
    }

    fn convert_u8_to_byte(&mut self, a: Target) -> ByteTarget {
        ByteTarget {
            bits: self.builder.split_le(a, 8).into_iter().rev().collect(),
        }
    }

    fn convert_byte_to_u8(&mut self, a: ByteTarget) -> Target {
        self.builder.le_sum(a.bits.into_iter().rev())
    }

    fn _translate_u8_bitwise_operation(self: &mut Self, lhs: ByteTarget, rhs: ByteTarget,
                                       operation: fn(&mut Self, BoolTarget, BoolTarget) -> BoolTarget) -> ByteTarget {
        ByteTarget {
            bits: lhs
                .bits.iter().zip(rhs.bits.iter())
                .map(|(x, y)| operation(self, *x, *y)).collect(),
        }
    }

    fn _register_public_parameters_from_acir_circuit(self: &mut Self, circuit: &Circuit) {
        let public_parameters_as_list: Vec<Witness> = circuit.public_parameters.0.iter().cloned().collect();
        for public_parameter_witness in public_parameters_as_list {
            self._register_new_public_input_from_witness(public_parameter_witness);
        }
    }

    fn _register_new_public_input_from_witness(self: &mut Self, public_input_witness: Witness) -> Target {
        let public_input_target = self.builder.add_virtual_target();
        self.builder.register_public_input(public_input_target);
        self.witness_target_map.insert(public_input_witness, public_input_target);
        public_input_target
    }

    fn _field_element_to_goldilocks_field(self: &mut Self, fe: &FieldElement) -> F {
        let fe_as_big_uint = BigUint::from_bytes_be(&fe.to_be_bytes() as &[u8]);
        F::from_noncanonical_biguint(fe_as_big_uint)
    }

    fn _register_intermediate_witnesses_for_assert_zero(self: &mut Self, expr: &Expression) {
        for (_, witness_1, witness_2) in &expr.mul_terms {
            self._get_or_create_target_for_witness(*witness_1);
            self._get_or_create_target_for_witness(*witness_2);
        }
        for (_, witness) in &expr.linear_combinations {
            self._get_or_create_target_for_witness(*witness);
        }
    }

    fn _register_intermediate_witnesses_for_memory_op(self: &mut Self, op: &MemOp) {
        let at = &op.index.linear_combinations[0].1;
        self._get_or_create_target_for_witness(*at);

        let value = &op.value.linear_combinations[0].1;
        self._get_or_create_target_for_witness(*value);
    }

    fn _get_or_create_target_for_witness(self: &mut Self, witness: Witness) -> Target {
        match self.witness_target_map.get(&witness) {
            Some(target) => *target,
            None => {
                let target = self.builder.add_virtual_target();
                self.witness_target_map.insert(witness, target);
                target
            }
        }
    }

    fn _translate_assert_zero(self: &mut Self, expression: &Expression) {
        let g_constant = self._field_element_to_goldilocks_field(&expression.q_c);

        let constant_target = self.builder.constant(g_constant);
        let mut current_acc_target = constant_target;
        current_acc_target = self._add_linear_combinations(expression, current_acc_target);
        current_acc_target = self._add_cuadratic_combinations(expression, current_acc_target);
        self.builder.assert_zero(current_acc_target);
    }

    fn _add_cuadratic_combinations(self: &mut Self, expression: &Expression, mut current_acc_target: Target) -> Target {
        let mul_terms = &expression.mul_terms;
        for mul_term in mul_terms {
            let (f_cuadratic_factor, public_input_witness_1, public_input_witness_2) = mul_term;
            let cuadratic_target = self._compute_cuadratic_term_target(f_cuadratic_factor, public_input_witness_1, public_input_witness_2);
            let new_target = self.builder.add(cuadratic_target, current_acc_target);
            current_acc_target = new_target;
        }
        current_acc_target
    }

    fn _add_linear_combinations(self: &mut Self, expression: &Expression, mut current_acc_target: Target) -> Target {
        let linear_combinations = &expression.linear_combinations;
        for (f_multiply_factor, public_input_witness) in linear_combinations {
            let linear_combination_target = self._compute_linear_combination_target(f_multiply_factor, public_input_witness);
            let new_target = self.builder.add(linear_combination_target, current_acc_target);
            current_acc_target = new_target;
        }
        current_acc_target
    }

    fn _compute_linear_combination_target(self: &mut Self,
                                          f_multiply_constant_factor: &FieldElement,
                                          public_input_witness: &Witness) -> Target {
        let factor_target = *self.witness_target_map.get(public_input_witness).unwrap();
        let g_first_pi_factor = self._field_element_to_goldilocks_field(f_multiply_constant_factor);
        self.builder.mul_const(g_first_pi_factor, factor_target)
    }

    fn _compute_cuadratic_term_target(self: &mut Self,
                                      f_cuadratic_factor: &FieldElement,
                                      public_input_witness_1: &Witness,
                                      public_input_witness_2: &Witness) -> Target {
        let g_cuadratic_factor = self._field_element_to_goldilocks_field(f_cuadratic_factor);
        let first_public_input_target = *self.witness_target_map.get(public_input_witness_1).unwrap();
        let second_public_input_target = *self.witness_target_map.get(public_input_witness_2).unwrap();

        let cuadratic_target = self.builder.mul(first_public_input_target, second_public_input_target);
        self.builder.mul_const(g_cuadratic_factor, cuadratic_target)
    }

    fn and(&mut self, b1: BoolTarget, b2: BoolTarget) -> BoolTarget {
        self.builder.and(b1, b2)
    }

    fn xor(&mut self, b1: BoolTarget, b2: BoolTarget) -> BoolTarget {
        // a xor b = (a or b) and (not (a and b))
        let b1_or_b2 = self.builder.or(b1, b2);
        let b1_and_b2 = self.builder.and(b1, b2);
        let not_b1_and_b2 = self.builder.not(b1_and_b2);
        self.builder.and(b1_or_b2, not_b1_and_b2)
    }
}

